use crate::input::RawPointer;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeRequest {
    Restack {
        operation: u64,
        bottom_to_top: [Option<u32>; 3],
    },
    SetOwner {
        operation: u64,
        window: Option<u32>,
        restore_focus: bool,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum NativeEvent {
    Ready,
    Applied {
        operation: u64,
        result: Result<(), String>,
    },
    Pointer {
        epoch: u64,
        input: RawPointer,
    },
    Wheel {
        modifier: String,
        delta: f32,
        root: (i16, i16),
    },
    Key(String),
    Escape,
    Lost {
        epoch: u64,
        reason: String,
    },
    Exited(Result<(), String>),
}

pub type NativeEventSink = std::sync::Arc<dyn Fn(NativeEvent) + Send + Sync>;

#[cfg(target_os = "linux")]
mod transport {
    use std::io::{self, Read, Write};
    use std::os::fd::{AsRawFd, RawFd};
    use std::os::unix::net::UnixStream;
    use std::sync::Arc;

    use crossbeam_channel::{Receiver, Sender, TryRecvError};

    use super::NativeRequest;

    #[derive(Clone)]
    pub struct NativeHandle {
        sender: Option<Sender<NativeRequest>>,
        wake: Arc<UnixStream>,
    }

    pub struct NativeInbox {
        receiver: Receiver<NativeRequest>,
        wake: UnixStream,
    }

    pub fn channel() -> io::Result<(NativeHandle, NativeInbox)> {
        let (writer, reader) = UnixStream::pair()?;
        writer.set_nonblocking(true)?;
        reader.set_nonblocking(true)?;
        let (sender, receiver) = crossbeam_channel::unbounded();
        Ok((
            NativeHandle {
                sender: Some(sender),
                wake: Arc::new(writer),
            },
            NativeInbox {
                receiver,
                wake: reader,
            },
        ))
    }

    impl NativeHandle {
        pub fn send(&self, request: NativeRequest) -> io::Result<()> {
            self.sender
                .as_ref()
                .expect("a live handle owns a sender")
                .send(request)
                .map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))?;
            notify(&self.wake)
        }
    }

    impl Drop for NativeHandle {
        fn drop(&mut self) {
            drop(self.sender.take());
            let _ = notify(&self.wake);
        }
    }

    impl NativeInbox {
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

        pub(crate) fn try_recv(&self) -> Result<NativeRequest, TryRecvError> {
            self.receiver.try_recv()
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
        use std::time::Duration;

        #[test]
        fn ordered_commands_wake_a_blocked_reader_after_queueing() {
            let (handle, mut inbox) = channel().unwrap();
            inbox.wake.set_nonblocking(false).unwrap();
            inbox
                .wake
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let producer = std::thread::spawn(move || {
                handle
                    .send(NativeRequest::SetOwner {
                        operation: 1,
                        window: None,
                        restore_focus: true,
                    })
                    .unwrap();
                handle
                    .send(NativeRequest::Restack {
                        operation: 2,
                        bottom_to_top: [Some(1), Some(2), None],
                    })
                    .unwrap();
            });
            assert_eq!(inbox.wake.read(&mut [0]).unwrap(), 1);
            inbox.drain_wake().unwrap();
            assert_eq!(
                inbox.try_recv(),
                Ok(NativeRequest::SetOwner {
                    operation: 1,
                    window: None,
                    restore_focus: true,
                })
            );
            assert_eq!(
                inbox.try_recv(),
                Ok(NativeRequest::Restack {
                    operation: 2,
                    bottom_to_top: [Some(1), Some(2), None],
                })
            );
            producer.join().unwrap();
        }

        #[test]
        fn final_handle_drop_wakes_disconnection() {
            let (handle, mut inbox) = channel().unwrap();
            inbox.wake.set_nonblocking(false).unwrap();
            inbox
                .wake
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            drop(handle);
            assert_eq!(inbox.wake.read(&mut [0]).unwrap(), 1);
            assert_eq!(inbox.try_recv(), Err(TryRecvError::Disconnected));
        }
    }
}

#[cfg(target_os = "linux")]
pub use transport::{NativeHandle, NativeInbox, channel};

#[cfg(not(target_os = "linux"))]
mod unsupported {
    use std::io;

    use super::NativeRequest;

    #[derive(Clone)]
    pub struct NativeHandle;
    pub struct NativeInbox;

    pub fn channel() -> io::Result<(NativeHandle, NativeInbox)> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "native input requires Linux",
        ))
    }

    impl NativeHandle {
        pub fn send(&self, _request: NativeRequest) -> io::Result<()> {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "native input requires Linux",
            ))
        }
    }
}

#[cfg(not(target_os = "linux"))]
pub use unsupported::{NativeHandle, NativeInbox, channel};
