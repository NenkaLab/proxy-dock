//! Listening sockets: AF_VSOCK (Hyper-V sockets as the Linux guest sees them) or, for tests, TCP on 127.0.0.1.

use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

/// A connected stream from the host.
pub trait Conn: Read + Write + Send {
    fn try_clone_conn(&self) -> io::Result<Box<dyn Conn>>;
    fn shutdown_write(&self);
}

impl Conn for TcpStream {
    fn try_clone_conn(&self) -> io::Result<Box<dyn Conn>> {
        Ok(Box::new(self.try_clone()?))
    }
    fn shutdown_write(&self) {
        let _ = self.shutdown(Shutdown::Write);
    }
}

/// A vsock stream (std has no type for it; read/write/shutdown are the plain socket calls).
pub struct Vsock(OwnedFd);

impl Read for Vsock {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = unsafe { libc::read(self.0.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
        if n < 0 { Err(io::Error::last_os_error()) } else { Ok(n as usize) }
    }
}

impl Write for Vsock {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = unsafe { libc::send(self.0.as_raw_fd(), buf.as_ptr().cast(), buf.len(), libc::MSG_NOSIGNAL) };
        if n < 0 { Err(io::Error::last_os_error()) } else { Ok(n as usize) }
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Conn for Vsock {
    fn try_clone_conn(&self) -> io::Result<Box<dyn Conn>> {
        Ok(Box::new(Vsock(self.0.try_clone()?)))
    }
    fn shutdown_write(&self) {
        unsafe { libc::shutdown(self.0.as_raw_fd(), libc::SHUT_WR) };
    }
}

pub enum Listener {
    Vsock(OwnedFd),
    Tcp(TcpListener),
}

impl Listener {
    pub fn bind(tcp: bool, port: u32) -> io::Result<Listener> {
        if tcp {
            return Ok(Listener::Tcp(TcpListener::bind(("127.0.0.1", port as u16))?));
        }
        unsafe {
            let fd = libc::socket(libc::AF_VSOCK, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0);
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            let fd = OwnedFd::from_raw_fd(fd);
            let mut addr: libc::sockaddr_vm = std::mem::zeroed();
            addr.svm_family = libc::AF_VSOCK as libc::sa_family_t;
            addr.svm_port = port;
            addr.svm_cid = libc::VMADDR_CID_ANY;
            let len = std::mem::size_of::<libc::sockaddr_vm>() as libc::socklen_t;
            if libc::bind(fd.as_raw_fd(), (&addr as *const libc::sockaddr_vm).cast(), len) < 0 {
                return Err(io::Error::last_os_error());
            }
            if libc::listen(fd.as_raw_fd(), 128) < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(Listener::Vsock(fd))
        }
    }

    pub fn accept(&self) -> io::Result<Box<dyn Conn>> {
        match self {
            Listener::Tcp(l) => {
                let (s, _) = l.accept()?;
                let _ = s.set_nodelay(true);
                Ok(Box::new(s))
            }
            Listener::Vsock(fd) => {
                let c = unsafe {
                    libc::accept4(fd.as_raw_fd(), std::ptr::null_mut(), std::ptr::null_mut(), libc::SOCK_CLOEXEC)
                };
                if c < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(Box::new(Vsock(unsafe { OwnedFd::from_raw_fd(c) })))
            }
        }
    }
}
