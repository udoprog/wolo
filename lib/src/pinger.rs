use core::ffi::{c_int, c_void};
use core::fmt;
use core::mem::{MaybeUninit, size_of, zeroed};
use core::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use core::ptr;
use core::sync::atomic::AtomicU16;

use std::io;
use std::net::{SocketAddr, SocketAddrV4, SocketAddrV6};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::sync::atomic::Ordering;

use tokio::io::Interest;
use tokio::io::unix::AsyncFd;

use crate::Buffer;
use crate::error::{Error, ErrorKind};
use crate::icmp;
use crate::ip;

macro_rules! rt {
    ($e:expr) => {{
        let n = $e;

        if n != 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }};
}

/// The response to a ping.
#[derive(Debug)]
#[non_exhaustive]
pub struct Response {
    pub outcome: Outcome,
    pub code: u8,
    pub source: IpAddr,
    pub dest: IpAddr,
    pub identifier: u16,
    pub sequence: u16,
    pub checksum: u16,
    pub expected_checksum: u16,
}

struct ErrorPayload {
    outcome: Option<Outcome>,
    code: u8,
}

#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub enum Outcome {
    /// Unknown V4 outcome.
    V4(icmp::v4::Type),
    /// Unknown V6 outcome.
    V6(icmp::v6::Type),
}

impl Outcome {
    /// Returns true if the outcome is an echo reply.
    pub fn is_echo_reply(&self) -> bool {
        match self {
            Outcome::V4(ty) => *ty == icmp::v4::Type::ECHO_REPLY,
            Outcome::V6(ty) => *ty == icmp::v6::Type::ECHO_REPLY,
        }
    }
}

impl fmt::Display for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Outcome::V4(ty) => ty.fmt(f),
            Outcome::V6(ty) => ty.fmt(f),
        }
    }
}

/// A helper structure for sending and handling pings.
pub struct Pinger {
    socket: AsyncFd<OwnedFd>,
    raw_socket: bool,
    seq: AtomicU16,
}

impl Pinger {
    /// Construct a ICMPv4 pinger.
    pub fn v4() -> Result<Self, Error> {
        Self::_inner(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0)))
    }

    /// Construct a ICMPv6 pinger.
    pub fn v6() -> Result<Self, Error> {
        Self::_inner(SocketAddr::V6(SocketAddrV6::new(
            Ipv6Addr::UNSPECIFIED,
            0,
            0,
            0,
        )))
    }

    fn _inner(addr: SocketAddr) -> Result<Self, Error> {
        let (domain, protocol, level, recv_err, packet_info) = match addr {
            SocketAddr::V4(..) => (
                libc::AF_INET,
                libc::IPPROTO_ICMP,
                libc::SOL_IP,
                libc::IP_RECVERR,
                libc::IP_PKTINFO,
            ),
            SocketAddr::V6(..) => (
                libc::AF_INET6,
                libc::IPPROTO_ICMPV6,
                libc::SOL_IPV6,
                libc::IPV6_RECVERR,
                libc::IPV6_RECVPKTINFO,
            ),
        };

        let socket = unsafe {
            let fd = libc::socket(domain, libc::SOCK_DGRAM, protocol);

            if fd < 0 {
                return Err(Error::new(ErrorKind::Socket(io::Error::last_os_error())));
            }

            OwnedFd::from_raw_fd(fd)
        };

        unsafe {
            let (addr, addr_len) = to_sockaddr(addr);

            rt!(libc::bind(
                socket.as_raw_fd(),
                &addr as *const _ as *const libc::sockaddr,
                addr_len,
            ))
            .map_err(ErrorKind::Bind)?;
        }

        set_nonblocking(&socket).map_err(ErrorKind::SetNonblocking)?;
        set_recv_err(&socket, level, recv_err).map_err(ErrorKind::SetRecvErr)?;
        set_packet_info(&socket, level, packet_info).map_err(ErrorKind::SetPacketInfo)?;

        Ok(Self {
            socket: AsyncFd::new(socket).map_err(ErrorKind::AsyncFd)?,
            raw_socket: false,
            seq: AtomicU16::new(0),
        })
    }

    /// Send a ping.
    ///
    /// To receive the response, call [`recv`].
    pub async fn ping(&self, buf: &mut Buffer, dest: IpAddr, data: &[u8]) -> Result<u16, Error> {
        match dest {
            IpAddr::V4(..) => self.ping_v4(buf, dest, data).await,
            IpAddr::V6(..) => self.ping_v6(buf, dest, data).await,
        }
    }

    fn next_seq(&self) -> u16 {
        self.seq.fetch_add(1, Ordering::Relaxed)
    }

    async fn ping_v4(&self, buf: &mut Buffer, dest: IpAddr, data: &[u8]) -> Result<u16, Error> {
        let sequence = self.next_seq();

        // NOTE: Checksum is calculated by the kernel for ICMPv4
        let mut header = icmp::v4::Header::ZEROED;
        header.ty = icmp::v4::Type::ECHO_REQUEST;
        header.set_sequence(sequence);

        buf.clear();
        buf.extend_from_slice(header.as_bytes());
        buf.extend_from_slice(data);

        self.send_to(buf.as_bytes(), dest).await?;
        Ok(sequence)
    }

    async fn ping_v6(&self, buf: &mut Buffer, dest: IpAddr, data: &[u8]) -> Result<u16, Error> {
        let sequence = self.next_seq();

        // NOTE: Checksum is calculated by the kernel for ICMPv6
        let mut header = icmp::v6::Header::ZEROED;
        header.ty = icmp::v6::Type::ECHO_REQUEST;
        header.set_sequence(sequence);

        buf.clear();
        buf.extend_from_slice(header.as_bytes());
        buf.extend_from_slice(data);

        self.send_to(buf.as_bytes(), dest).await?;
        Ok(sequence)
    }

    async fn send_to(&self, buf: &[u8], dest: IpAddr) -> Result<usize, Error> {
        unsafe {
            let (addr, addr_len) = to_sockaddr(SocketAddr::new(dest, 0));

            let n = self
                .socket
                .async_io(Interest::WRITABLE, |socket| {
                    let n = libc::sendto(
                        socket.as_raw_fd(),
                        buf.as_ptr().cast::<c_void>(),
                        buf.len(),
                        0,
                        &addr as *const _ as *const libc::sockaddr,
                        addr_len,
                    );

                    if n < 0 {
                        return Err(io::Error::last_os_error());
                    }

                    Ok(n as usize)
                })
                .await
                .map_err(ErrorKind::SendTo)?;

            Ok(n)
        }
    }

    unsafe fn recv_from(
        fd: RawFd,
        buf: &mut Buffer,
        error: &mut ErrorPayload,
        dest: &mut Option<IpAddr>,
        flags: c_int,
    ) -> io::Result<SocketAddr> {
        unsafe {
            let mut msghdr = zeroed::<libc::msghdr>();

            let mut iov = libc::iovec {
                iov_base: buf.as_uninit_mut().as_mut_ptr().cast(),
                iov_len: buf.remaining_mut(),
            };

            msghdr.msg_iov = &mut iov;
            msghdr.msg_iovlen = 1;

            let mut control = Buffer::new();

            msghdr.msg_control = control.as_uninit_mut().as_mut_ptr().cast();
            msghdr.msg_controllen = control.remaining_mut();

            let mut sock_addr = zeroed::<libc::sockaddr_storage>();
            msghdr.msg_name = (&mut sock_addr as *mut libc::sockaddr_storage).cast();
            msghdr.msg_namelen = size_of::<libc::sockaddr_storage>() as libc::socklen_t;

            let n = libc::recvmsg(fd, &mut msghdr, flags);

            if n < 0 {
                let err = io::Error::last_os_error();
                return Err(err);
            }

            let mut cur = libc::CMSG_FIRSTHDR(&msghdr);

            while let Some(cmsg) = cur.as_mut() {
                match (cmsg.cmsg_level, cmsg.cmsg_type) {
                    (libc::SOL_IP, libc::IP_RECVERR) => {
                        let data = &*libc::CMSG_DATA(cmsg)
                            .cast_const()
                            .cast::<libc::sock_extended_err>();
                        let ty = icmp::v4::Type::new(data.ee_type as u8);
                        error.outcome = Some(Outcome::V4(ty));
                        error.code = data.ee_code;
                    }
                    (libc::SOL_IPV6, libc::IPV6_RECVERR) => {
                        let data = &*libc::CMSG_DATA(cmsg)
                            .cast_const()
                            .cast::<libc::sock_extended_err>();
                        let ty = icmp::v6::Type::new(data.ee_type as u8);
                        error.outcome = Some(Outcome::V6(ty));
                        error.code = data.ee_code;
                    }
                    (libc::SOL_IP, libc::IP_PKTINFO) => {
                        let data = &*libc::CMSG_DATA(cmsg)
                            .cast_const()
                            .cast::<libc::in_pktinfo>();

                        dest.replace(IpAddr::V4(Ipv4Addr::from_bits(
                            data.ipi_addr.s_addr.to_be(),
                        )));
                    }
                    (libc::SOL_IPV6, libc::IPV6_PKTINFO) => {
                        let data = &*libc::CMSG_DATA(cmsg)
                            .cast_const()
                            .cast::<libc::in6_pktinfo>();

                        dest.replace(IpAddr::V6(Ipv6Addr::from_octets(data.ipi6_addr.s6_addr)));
                    }
                    _ => {
                        println!("unmatched");
                    }
                }

                cur = libc::CMSG_NXTHDR(&msghdr, cmsg);
            }

            buf.advance(n as usize);
            from_sockaddr(&sock_addr)
        }
    }

    /// Receive an ICMP error message.
    pub async fn recv(&self, buf: &mut Buffer) -> Result<Response, Error> {
        const INTEREST: Interest = Interest::READABLE
            .add(Interest::ERROR)
            .add(Interest::PRIORITY);

        buf.clear();

        let mut error = ErrorPayload {
            outcome: None,
            code: 0,
        };

        let mut dest = None;

        let (source, readable) = loop {
            let mut ready = self
                .socket
                .ready(INTEREST)
                .await
                .map_err(ErrorKind::RecvFromReady)?;

            let mut flags = 0;
            let readable = ready.ready().is_readable();

            // If we are not reading, then we are making a call to the error
            // queue.
            if !readable {
                flags |= libc::MSG_ERRQUEUE;
            }

            let result = unsafe {
                Self::recv_from(
                    ready.get_ref().as_raw_fd(),
                    buf,
                    &mut error,
                    &mut dest,
                    flags,
                )
            };

            match result {
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                    ready.clear_ready();
                }
                Err(err) => return Err(Error::new(ErrorKind::RecvFrom(err))),
                Ok(addr) => break (addr, readable),
            }
        };

        let Some(dest) = dest else {
            return Err(Error::new(ErrorKind::RecvMissingDestinationAddress));
        };

        if readable {
            let checksum = match (&dest, &source) {
                (IpAddr::V6(dest), SocketAddr::V6(addr)) => {
                    icmp::v6::checksum(dest, addr.ip(), buf.as_bytes())
                }
                _ => icmp::v4::checksum(buf.as_bytes()),
            };

            self.decode_response(buf, source.ip(), dest, checksum)
        } else {
            let Some(outcome) = error.outcome else {
                return Err(Error::new(ErrorKind::RecvErrorMissingOutcome));
            };

            // Decode the original response so we can access the payload.
            _ = self.decode_response(buf, source.ip(), dest, 0)?;

            Ok(Response {
                outcome,
                code: error.code,
                source: source.ip(),
                dest,
                identifier: 0,
                sequence: 0,
                checksum: 0,
                expected_checksum: 0,
            })
        }
    }

    fn decode_response(
        &self,
        buf: &mut Buffer,
        source: IpAddr,
        dest: IpAddr,
        expected_checksum: u16,
    ) -> Result<Response, Error> {
        let outcome;
        let code;
        let checksum;
        let identifier;
        let sequence;

        match source {
            IpAddr::V4(..) => {
                if self.raw_socket {
                    let ip = buf.read::<ip::v4::Header>()?;

                    if ip.version() != 4 {
                        return Err(Error::new(ErrorKind::IpVersionMismatch {
                            actual: ip.version(),
                            expected: 4,
                        }));
                    }

                    if ip.protocol() != libc::IPPROTO_ICMP {
                        return Err(Error::new(ErrorKind::ProtocolMismatch {
                            actual: ip.protocol(),
                            expected: libc::IPPROTO_ICMP,
                        }));
                    }
                }

                let header = buf.read::<icmp::v4::Header>()?;

                outcome = Outcome::V4(header.ty);
                code = header.code;
                checksum = header.checksum();
                identifier = header.identifier();
                sequence = header.sequence();
            }
            IpAddr::V6(..) => {
                let header = buf.read::<icmp::v6::Header>()?;
                outcome = Outcome::V6(header.ty);
                code = header.code;
                checksum = header.checksum();
                identifier = header.identifier();
                sequence = header.sequence();
            }
        }

        Ok(Response {
            outcome,
            code,
            source,
            dest,
            identifier,
            sequence,
            checksum,
            expected_checksum,
        })
    }
}

fn to_sockaddr(addr: SocketAddr) -> (libc::sockaddr_storage, libc::socklen_t) {
    const {
        assert!(size_of::<libc::sockaddr_storage>() >= size_of::<libc::sockaddr_in>());
        assert!(size_of::<libc::sockaddr_storage>() >= size_of::<libc::sockaddr_in6>());
    }

    // SAFETY: We are initializing the sockaddr structures properly.
    unsafe {
        let mut addr_base = MaybeUninit::<libc::sockaddr_storage>::uninit();

        let (addr, addr_len) = match addr {
            SocketAddr::V4(a) => {
                addr_base
                    .as_mut_ptr()
                    .cast::<libc::sockaddr_in>()
                    .write(libc::sockaddr_in {
                        sin_family: libc::AF_INET as libc::sa_family_t,
                        sin_port: a.port().to_be(),
                        sin_addr: libc::in_addr {
                            s_addr: a.ip().to_bits().to_be(),
                        },
                        sin_zero: [0; 8],
                    });

                let addr_len = size_of::<libc::sockaddr_in>() as libc::socklen_t;
                (addr_base.assume_init(), addr_len)
            }
            SocketAddr::V6(a) => {
                addr_base
                    .as_mut_ptr()
                    .cast::<libc::sockaddr_in6>()
                    .write(libc::sockaddr_in6 {
                        sin6_family: libc::AF_INET6 as libc::sa_family_t,
                        sin6_port: a.port().to_be(),
                        sin6_flowinfo: 0,
                        sin6_addr: libc::in6_addr {
                            s6_addr: a.ip().octets(),
                        },
                        sin6_scope_id: a.scope_id(),
                    });

                let addr_len = size_of::<libc::sockaddr_in6>() as libc::socklen_t;
                (addr_base.assume_init(), addr_len)
            }
        };

        (addr, addr_len)
    }
}

unsafe fn from_sockaddr(addr: *const libc::sockaddr_storage) -> io::Result<SocketAddr> {
    // SAFETY: We are assuming the storage is initialized.
    unsafe {
        let family = ptr::addr_of!((*addr.cast::<libc::sockaddr>()).sa_family).read();

        match family as c_int {
            libc::AF_INET => {
                let addr_in = &*(addr as *const libc::sockaddr).cast::<libc::sockaddr_in>();

                let ip = Ipv4Addr::from(u32::from_be(addr_in.sin_addr.s_addr));
                let port = u16::from_be(addr_in.sin_port);

                Ok(SocketAddr::V4(SocketAddrV4::new(ip, port)))
            }
            libc::AF_INET6 => {
                let addr = &*(addr as *const libc::sockaddr).cast::<libc::sockaddr_in6>();

                let ip = Ipv6Addr::from(addr.sin6_addr.s6_addr);
                let port = u16::from_be(addr.sin6_port);

                Ok(SocketAddr::V6(SocketAddrV6::new(
                    ip,
                    port,
                    addr.sin6_flowinfo,
                    addr.sin6_scope_id,
                )))
            }
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unsupported address family",
            )),
        }
    }
}

fn set_nonblocking(socket: &OwnedFd) -> io::Result<()> {
    unsafe {
        let flags = libc::fcntl(socket.as_raw_fd(), libc::F_GETFL, 0);

        if flags < 0 {
            return Err(io::Error::last_os_error());
        }

        rt!(libc::fcntl(
            socket.as_raw_fd(),
            libc::F_SETFL,
            flags | libc::O_NONBLOCK
        ))
    }
}

fn set_recv_err(socket: &OwnedFd, level: c_int, recv_err: i32) -> io::Result<()> {
    unsafe {
        let on: c_int = 1;

        rt!(libc::setsockopt(
            socket.as_raw_fd(),
            level,
            recv_err,
            (&on as *const c_int).cast(),
            size_of::<c_int>() as libc::socklen_t,
        ))
    }
}

fn set_packet_info(socket: &OwnedFd, level: c_int, packet_info: c_int) -> io::Result<()> {
    unsafe {
        let on: c_int = 1;

        rt!(libc::setsockopt(
            socket.as_raw_fd(),
            level,
            packet_info,
            (&on as *const c_int).cast(),
            size_of::<c_int>() as libc::socklen_t,
        ))
    }
}
