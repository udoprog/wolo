use std::io;
use std::net::Ipv4Addr;

use tokio::net::{ToSocketAddrs, UdpSocket};

const BROADCAST_TO_ADDR: (Ipv4Addr, u16) = (Ipv4Addr::new(255, 255, 255, 255), 9);
const BROADCAST_FROM_ADDR: (Ipv4Addr, u16) = (Ipv4Addr::new(0, 0, 0, 0), 0);
const MAGIC_BYTES_HEADER: [u8; 6] = [0xFF; 6];

#[repr(C)]
pub struct MagicPacket {
    header: [u8; 6],
    dest: [[u8; 6]; 16],
}

const _: () = const {
    assert!(core::mem::size_of::<MagicPacket>() == 102);
};

impl MagicPacket {
    /// Creates a new `MagicPacket` intended for `mac_address` (but doesn't send it yet).
    pub fn new(mac_address: [u8; 6]) -> MagicPacket {
        let mut magic_bytes = MagicPacket {
            header: MAGIC_BYTES_HEADER,
            dest: [[0u8; 6]; 16],
        };

        for d in magic_bytes.dest.iter_mut() {
            *d = mac_address;
        }

        magic_bytes
    }

    fn as_bytes(&self) -> &[u8; 102] {
        // SAFETY: `MagicPacket` is `repr(C)` and consists entirely of `u8`
        // arrays.
        unsafe { &*(self as *const MagicPacket as *const [u8; 102]) }
    }

    /// Sends the magic packet via UDP to the broadcast address
    /// `255.255.255.255:9`. Lets the operating system choose the source port
    /// and network interface.
    pub async fn send(&self) -> io::Result<()> {
        self.send_to(BROADCAST_TO_ADDR, BROADCAST_FROM_ADDR).await
    }

    /// Sends the magic packet via UDP to/from an IP address and port number of
    /// your choosing.
    pub async fn send_to(
        &self,
        to_addr: impl ToSocketAddrs,
        from_addr: impl ToSocketAddrs,
    ) -> io::Result<()> {
        let socket = UdpSocket::bind(from_addr).await?;
        socket.set_broadcast(true)?;
        socket.send_to(self.as_bytes(), to_addr).await?;
        Ok(())
    }
}
