#[cfg(target_os = "linux")]
mod linux {
    use std::io::{self, Read, Write};
    use std::os::fd::{AsRawFd, RawFd};
    use std::os::unix::net::UnixStream;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    use crossbeam_channel::{Receiver, Sender, TryRecvError};

    use crate::session::ZoomRequest;

    #[derive(Clone)]
    pub struct ZoomHandle {
        sender: Option<Sender<ZoomRequest>>,
        wake: Arc<UnixStream>,
        active: Arc<AtomicBool>,
    }

    pub struct ZoomInbox {
        receiver: Receiver<ZoomRequest>,
        wake: UnixStream,
        active: Arc<AtomicBool>,
    }

    pub fn channel() -> io::Result<(ZoomHandle, ZoomInbox)> {
        let (writer, reader) = UnixStream::pair()?;
        writer.set_nonblocking(true)?;
        reader.set_nonblocking(true)?;
        let (sender, receiver) = crossbeam_channel::unbounded();
        let active = Arc::new(AtomicBool::new(false));
        Ok((
            ZoomHandle {
                sender: Some(sender),
                wake: Arc::new(writer),
                active: Arc::clone(&active),
            },
            ZoomInbox {
                receiver,
                wake: reader,
                active,
            },
        ))
    }

    impl ZoomHandle {
        pub fn send(&self, request: ZoomRequest) -> io::Result<()> {
            self.sender
                .as_ref()
                .expect("a live handle owns a sender")
                .send(request)
                .map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))?;
            notify(&self.wake)
        }

        pub fn is_active(&self) -> bool {
            self.active.load(Ordering::Acquire)
        }
    }

    impl Drop for ZoomHandle {
        fn drop(&mut self) {
            drop(self.sender.take());
            let _ = notify(&self.wake);
        }
    }

    impl ZoomInbox {
        pub(crate) fn wake_fd(&self) -> RawFd {
            self.wake.as_raw_fd()
        }

        pub(crate) fn drain_wake(&mut self) -> io::Result<()> {
            let mut bytes = [0; 1024];
            loop {
                match self.wake.read(&mut bytes) {
                    Ok(0) => return Ok(()),
                    Ok(_) => {}
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
                    Err(error) => return Err(error),
                }
            }
        }

        pub(crate) fn try_recv(&self) -> Result<ZoomRequest, TryRecvError> {
            self.receiver.try_recv()
        }

        pub(crate) fn publish_active(&self, active: bool) {
            self.active.store(active, Ordering::Release);
        }
    }

    impl Drop for ZoomInbox {
        fn drop(&mut self) {
            self.publish_active(false);
        }
    }

    fn notify(stream: &UnixStream) -> io::Result<()> {
        let mut writer = stream;
        loop {
            match writer.write(&[1]) {
                Ok(1) => return Ok(()),
                Ok(_) => return Err(io::ErrorKind::WriteZero.into()),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
                Err(error) => return Err(error),
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::command::ZoomCommand;
        use std::time::Duration;

        #[test]
        fn wake_then_drain_preserves_command_order() {
            let (handle, mut inbox) = channel().unwrap();
            assert!(inbox.wake_fd() >= 0);
            for command in [
                ZoomCommand::On,
                ZoomCommand::Reset,
                ZoomCommand::Off,
                ZoomCommand::Toggle,
            ] {
                handle.send(ZoomRequest::Command(command)).unwrap();
            }
            inbox.drain_wake().unwrap();
            for command in [
                ZoomCommand::On,
                ZoomCommand::Reset,
                ZoomCommand::Off,
                ZoomCommand::Toggle,
            ] {
                assert_eq!(inbox.try_recv(), Ok(ZoomRequest::Command(command)));
            }
            assert_eq!(inbox.try_recv(), Err(TryRecvError::Empty));
            assert_eq!(
                inbox.wake.read(&mut [0]).unwrap_err().kind(),
                io::ErrorKind::WouldBlock
            );
        }

        #[test]
        fn full_wake_socket_does_not_lose_a_request() {
            let (handle, mut inbox) = channel().unwrap();
            let mut writer = &*handle.wake;
            let bytes = [1; 4096];
            loop {
                match writer.write(&bytes) {
                    Ok(count) => assert!(count > 0),
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                    Err(error) => panic!("filling wake socket failed: {error}"),
                }
            }
            handle
                .send(ZoomRequest::Command(ZoomCommand::Reset))
                .unwrap();
            inbox.drain_wake().unwrap();
            assert_eq!(
                inbox.try_recv(),
                Ok(ZoomRequest::Command(ZoomCommand::Reset))
            );
        }

        #[test]
        fn concurrent_producers_preserve_each_producers_order() {
            let (handle, mut inbox) = channel().unwrap();
            let producers: Vec<_> = (0..4)
                .map(|producer| {
                    let handle = handle.clone();
                    std::thread::spawn(move || {
                        for sequence in 0..128 {
                            handle
                                .send(ZoomRequest::Scroll {
                                    delta: 1.0,
                                    root: (producer, sequence),
                                })
                                .unwrap();
                        }
                    })
                })
                .collect();
            for producer in producers {
                producer.join().unwrap();
            }
            inbox.drain_wake().unwrap();
            let mut next = [0; 4];
            for _ in 0..512 {
                let ZoomRequest::Scroll {
                    root: (producer, sequence),
                    ..
                } = inbox.try_recv().unwrap()
                else {
                    panic!("expected a scroll request");
                };
                assert_eq!(sequence, next[producer as usize]);
                next[producer as usize] += 1;
            }
            assert_eq!(next, [128; 4]);
            assert_eq!(inbox.try_recv(), Err(TryRecvError::Empty));
        }

        #[test]
        fn request_wakes_a_blocked_reader_after_it_is_queued() {
            let (handle, mut inbox) = channel().unwrap();
            inbox.wake.set_nonblocking(false).unwrap();
            inbox
                .wake
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let producer = std::thread::spawn(move || {
                handle.send(ZoomRequest::Command(ZoomCommand::On)).unwrap()
            });
            assert_eq!(inbox.wake.read(&mut [0]).unwrap(), 1);
            assert_eq!(inbox.try_recv(), Ok(ZoomRequest::Command(ZoomCommand::On)));
            producer.join().unwrap();
        }

        #[test]
        fn final_handle_drop_notifies_after_channel_disconnection() {
            let (handle, mut inbox) = channel().unwrap();
            let last = handle.clone();
            drop(handle);
            inbox.drain_wake().unwrap();
            assert_eq!(inbox.try_recv(), Err(TryRecvError::Empty));
            drop(last);
            assert_eq!(inbox.wake.read(&mut [0]).unwrap(), 1);
            assert_eq!(inbox.try_recv(), Err(TryRecvError::Disconnected));
        }

        #[test]
        fn only_the_inbox_publishes_active_status_and_clears_it_on_drop() {
            let (handle, inbox) = channel().unwrap();
            let cloned = handle.clone();
            handle.send(ZoomRequest::Command(ZoomCommand::On)).unwrap();
            assert!(!handle.is_active());
            inbox.publish_active(true);
            assert!(handle.is_active());
            assert!(cloned.is_active());
            inbox.publish_active(false);
            assert!(!handle.is_active());
            inbox.publish_active(true);
            drop(inbox);
            assert!(!handle.is_active());
            assert!(!cloned.is_active());
        }
    }
}

#[cfg(target_os = "linux")]
pub use linux::{ZoomHandle, ZoomInbox, channel};

#[cfg(not(target_os = "linux"))]
mod unsupported {
    use std::io;

    use crate::session::ZoomRequest;

    #[derive(Clone)]
    pub struct ZoomHandle;
    pub struct ZoomInbox;

    pub fn channel() -> io::Result<(ZoomHandle, ZoomInbox)> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "zoom transport requires Linux",
        ))
    }

    impl ZoomHandle {
        pub fn send(&self, _request: ZoomRequest) -> io::Result<()> {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "zoom transport requires Linux",
            ))
        }

        pub fn is_active(&self) -> bool {
            false
        }
    }
}

#[cfg(not(target_os = "linux"))]
pub use unsupported::{ZoomHandle, ZoomInbox, channel};
