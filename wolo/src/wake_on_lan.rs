use core::mem::size_of;

use core::net::SocketAddrV4;
use std::io;
use std::net::Ipv4Addr;

use macaddr::MacAddr6;
use tokio::net::UdpSocket;

const FROM: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0);
const TO: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::BROADCAST, 9);
const MAGIC_BYTES_HEADER: [u8; 6] = [0xFF; 6];

/// Configure a broadcast socket used for sending Wake-on-LAN magic packets.
pub struct BroadcastSocket {
    socket: UdpSocket,
}

impl BroadcastSocket {
    /// Creates a new UDP socket bound to `from` that can send broadcast
    /// messages.
    pub async fn bind() -> io::Result<Self> {
        let socket = UdpSocket::bind(FROM).await?;
        socket.set_broadcast(true)?;
        Ok(Self { socket })
    }

    /// Sends the given magic packet via this socket to the broadcast address.
    pub async fn send(&self, packet: &MagicPacket) -> io::Result<()> {
        self.socket.send_to(packet.as_bytes(), TO).await?;
        Ok(())
    }
}

#[repr(C)]
pub struct MagicPacket {
    // 6 bytes of 0xFF.
    header: [u8; 6],
    // 16 repetitions of the target MAC address.
    dest: [[u8; 6]; 16],
}

const _: () = const {
    assert!(size_of::<MagicPacket>() == 102);
};

impl MagicPacket {
    /// Creates a new `MagicPacket` intended for `mac_address` (but doesn't send it yet).
    pub fn new(address: MacAddr6) -> Self {
        let mut dest = [[0u8; 6]; 16];

        for d in dest.iter_mut() {
            *d = address.into_array();
        }

        Self {
            header: MAGIC_BYTES_HEADER,
            dest,
        }
    }

    fn as_bytes(&self) -> &[u8] {
        // SAFETY: `MagicPacket` is `repr(C)` and consists entirely of `u8`
        // arrays.
        unsafe { &*(self as *const Self as *const [u8; size_of::<Self>()]) }
    }
}
