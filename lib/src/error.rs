use core::ffi::c_int;
use core::fmt;

use std::io;

/// An error that can occur when handling ICMP packets.
pub struct Error {
    kind: ErrorKind,
}

impl Error {
    #[inline]
    pub(super) fn new(kind: ErrorKind) -> Self {
        Self { kind }
    }
}

impl fmt::Debug for Error {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.kind.fmt(f)
    }
}

impl fmt::Display for Error {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.kind.fmt(f)
    }
}

impl From<ErrorKind> for Error {
    #[inline]
    fn from(kind: ErrorKind) -> Self {
        Self::new(kind)
    }
}

#[derive(Debug)]
pub(super) enum ErrorKind {
    AsyncFd(io::Error),
    Socket(io::Error),
    SetNonblocking(io::Error),
    Bind(io::Error),
    SendTo(io::Error),
    RecvFromReady(io::Error),
    RecvFrom(io::Error),
    SetRecvErr(io::Error),
    SetPacketInfo(io::Error),
    BufferTooSmall { actual: usize, needed: usize },
    IpVersionMismatch { actual: u8, expected: u8 },
    ProtocolMismatch { actual: c_int, expected: c_int },
    RecvMissingDestinationAddress,
    RecvErrorMissingOutcome,
}

impl fmt::Display for ErrorKind {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AsyncFd(..) => write!(f, "Building asynchronous fd failed"),
            Self::Socket(..) => write!(f, "Creating socket failed"),
            Self::SetNonblocking(..) => write!(f, "Failed to set socket nonblocking"),
            Self::Bind(..) => write!(f, "Failed to bind socket"),
            Self::SendTo(..) => write!(f, "Failed to send to socket"),
            Self::RecvFromReady(..) => write!(f, "Failed to await socket recv readiness"),
            Self::RecvFrom(..) => write!(f, "Failed to receive from socket"),
            Self::SetRecvErr(..) => write!(f, "Failed to set socket recv error option"),
            Self::SetPacketInfo(..) => write!(f, "Failed to set socket packet info option"),
            Self::BufferTooSmall { actual, needed } => {
                write!(f, "Buffer {actual} too small for read up to byte {needed}")
            }
            Self::IpVersionMismatch { actual, expected } => {
                write!(f, "IP version mismatch: expected {expected}, got {actual}")
            }
            Self::ProtocolMismatch { actual, expected } => {
                write!(
                    f,
                    "IP protocol mismatch: expected {expected:?}, got {actual:?}"
                )
            }
            Self::RecvMissingDestinationAddress => {
                write!(f, "Received ICMP message is missing destination address")
            }
            Self::RecvErrorMissingOutcome => {
                write!(
                    f,
                    "Received ICMP error message is missing outcome information"
                )
            }
        }
    }
}

impl core::error::Error for Error {
    #[inline]
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match &self.kind {
            ErrorKind::AsyncFd(e) => Some(e),
            ErrorKind::Socket(e) => Some(e),
            ErrorKind::SetNonblocking(e) => Some(e),
            ErrorKind::Bind(e) => Some(e),
            ErrorKind::SendTo(e) => Some(e),
            ErrorKind::RecvFromReady(e) => Some(e),
            ErrorKind::RecvFrom(e) => Some(e),
            ErrorKind::SetRecvErr(e) => Some(e),
            ErrorKind::SetPacketInfo(e) => Some(e),
            _ => None,
        }
    }
}
