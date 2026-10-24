#[cfg(target_os = "linux")]
mod linux {
    use std::io::{self, Read, Write};
    use std::os::fd::{AsRawFd, RawFd};
    use std::os::unix::net::UnixStream;
    use std::sync::Arc;

    use crossbeam_channel::{Receiver, Sender, TryRecvError};

    use crate::runtime::AnnotationRequest;

    #[derive(Clone)]
    pub struct AnnotationHandle {
        sender: Option<Sender<AnnotationRequest>>,
        wake: Arc<UnixStream>,
    }

    pub struct AnnotationInbox {
        receiver: Receiver<AnnotationRequest>,
        wake: UnixStream,
    }

    pub fn channel() -> io::Result<(AnnotationHandle, AnnotationInbox)> {
        let (writer, reader) = UnixStream::pair()?;
        writer.set_nonblocking(true)?;
        reader.set_nonblocking(true)?;
        let (sender, receiver) = crossbeam_channel::unbounded();
        Ok((
            AnnotationHandle {
                sender: Some(sender),
                wake: Arc::new(writer),
            },
            AnnotationInbox {
                receiver,
                wake: reader,
            },
        ))
    }

    impl AnnotationHandle {
        pub fn send(&self, request: AnnotationRequest) -> io::Result<()> {
            self.sender
                .as_ref()
                .expect("a live handle owns a sender")
                .send(request)
                .map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))?;
            notify(&self.wake)
        }
    }

    impl Drop for AnnotationHandle {
        fn drop(&mut self) {
            drop(self.sender.take());
            let _ = notify(&self.wake);
        }
    }

    impl AnnotationInbox {
        pub(crate) fn wake_fd(&self) -> RawFd {
            self.wake.as_raw_fd()
        }

        pub fn drain_wake(&mut self) -> io::Result<()> {
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

        pub fn try_recv(&self) -> Result<AnnotationRequest, TryRecvError> {
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
        use crate::{AnnotationCommand, AnnotationRequest};

        #[test]
        fn wake_transport_preserves_commands_before_close() {
            let (handle, mut inbox) = channel().unwrap();
            handle
                .send(AnnotationRequest::Command(AnnotationCommand::On))
                .unwrap();
            handle
                .send(AnnotationRequest::Command(AnnotationCommand::Off))
                .unwrap();
            drop(handle);
            inbox.drain_wake().unwrap();
            assert_eq!(
                inbox.try_recv().unwrap(),
                AnnotationRequest::Command(AnnotationCommand::On)
            );
            assert_eq!(
                inbox.try_recv().unwrap(),
                AnnotationRequest::Command(AnnotationCommand::Off)
            );
            assert!(inbox.try_recv().is_err());
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
                .send(AnnotationRequest::Command(AnnotationCommand::Toggle))
                .unwrap();
            inbox.drain_wake().unwrap();
            assert_eq!(
                inbox.try_recv(),
                Ok(AnnotationRequest::Command(AnnotationCommand::Toggle))
            );
        }

        #[test]
        fn cloned_handles_keep_the_transport_alive_until_the_last_drop() {
            let (handle, mut inbox) = channel().unwrap();
            let clone = handle.clone();
            drop(handle);
            clone
                .send(AnnotationRequest::Command(AnnotationCommand::Undo))
                .unwrap();
            inbox.drain_wake().unwrap();
            assert_eq!(
                inbox.try_recv(),
                Ok(AnnotationRequest::Command(AnnotationCommand::Undo))
            );
            drop(clone);
            assert!(inbox.wake_fd() >= 0);
            assert!(inbox.try_recv().is_err());
        }
    }
}

#[cfg(target_os = "linux")]
pub use linux::{AnnotationHandle, AnnotationInbox, channel};

#[cfg(not(target_os = "linux"))]
mod unsupported {
    use std::io;

    use crate::runtime::AnnotationRequest;

    #[derive(Clone)]
    pub struct AnnotationHandle;
    pub struct AnnotationInbox;

    pub fn channel() -> io::Result<(AnnotationHandle, AnnotationInbox)> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "annotation transport requires Linux",
        ))
    }

    impl AnnotationHandle {
        pub fn send(&self, _request: AnnotationRequest) -> io::Result<()> {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "annotation transport requires Linux",
            ))
        }
    }

    impl AnnotationInbox {
        pub fn drain_wake(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
}

#[cfg(not(target_os = "linux"))]
pub use unsupported::{AnnotationHandle, AnnotationInbox, channel};
