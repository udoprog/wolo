use core::fmt;
use core::mem::{MaybeUninit, size_of};
use core::net::Ipv6Addr;
use core::slice;

use crate::buf::Aligned;

/// The type of an ICMP message.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct Type(u8);

super::macros::define_types! {
    impl Type {
        /// destination host unreachable
        UNREACHABLE = 1;
        /// echo request
        ECHO_REQUEST = 128;
        /// echo reply
        ECHO_REPLY = 129;
    }
}

/// The code for a Destination Unreachable ICMP message.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Unreachable(u8);

super::macros::define_types! {
    impl Unreachable {
        /// no route to the destination
        NO_ROUTE = 0;
        /// communication with destination administratively prohibited
        ADMINISTRATIVELY_PROHIBITED = 1;
        /// beyond scope of source address
        BEYOND_SCOPE = 2;
        /// address unreachable
        ADDRESS_UNREACHABLE = 3;
        /// port unreachable
        PORT_UNREACHABLE = 4;
        /// source address failed ingress/egress policy
        SOURCE_POLICY_FAILED = 5;
        /// reject route to the destination
        ROUTE_REJECTED = 6;
        /// error in source routing header
        HEADER_ERROR = 7;
        /// headers too long
        HEADER_LENGTH = 8;
    }
}

unsafe impl Aligned for Header {}

/// The ICMP header structure.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct Header {
    pub ty: Type,
    pub code: u8,
    checksum: u16,
    identifier: u16,
    sequence: u16,
}

impl Header {
    /// A header with all fields set to zero.
    pub const ZEROED: Self = Self::from_array([0u8; Self::SIZE]);
    /// The size of the header in bytes.
    pub const SIZE: usize = size_of::<Self>();

    /// Read the given array as a Header.
    pub const fn from_array(buffer: [u8; Self::SIZE]) -> Self {
        let mut header = MaybeUninit::<Self>::uninit();

        unsafe {
            header
                .as_mut_ptr()
                .cast::<u8>()
                .copy_from_nonoverlapping(buffer.as_ptr(), size_of::<Self>());

            header.assume_init()
        }
    }

    /// Get the checksum from the header.
    #[inline]
    pub fn checksum(&self) -> u16 {
        u16::from_be(self.checksum)
    }

    /// Get the identifier from the header.
    #[inline]
    pub fn identifier(&self) -> u16 {
        u16::from_be(self.identifier)
    }

    /// Get the sequence number from the header.
    #[inline]
    pub fn sequence(&self) -> u16 {
        u16::from_be(self.sequence)
    }

    /// Set the sequence number in the header.
    #[inline]
    pub fn set_sequence(&mut self, sequence: u16) {
        self.sequence = sequence.to_be();
    }

    /// Get the header as a byte slice.
    pub fn as_bytes(&mut self) -> &[u8] {
        // SAFETY: The layout for Header is compatible with a byte slice of its
        // size.
        unsafe { slice::from_raw_parts((self as *const Self).cast::<u8>(), size_of::<Self>()) }
    }
}

/// Sum a byte slice as 16-bit big-endian words, padding if needed.
fn sum_be16(data: &[u8]) -> u64 {
    let mut sum: u64 = 0;

    let mut chunks = data.chunks_exact(2);

    for chunk in chunks.by_ref() {
        let word = u16::from_be_bytes([chunk[0], chunk[1]]);
        sum += word as u64;
    }

    if let &[last] = chunks.remainder() {
        let word = u16::from_be_bytes([last, 0]);
        sum += word as u64;
    }

    sum
}

pub fn checksum(src: &Ipv6Addr, dst: &Ipv6Addr, icmp: &[u8]) -> u16 {
    const NEXT_HEADER_ICMPV6: u8 = 58;

    let len_bytes = (icmp.len() as u32).to_be_bytes();
    let nh_bytes = [0, NEXT_HEADER_ICMPV6];

    let mut sum: u64 = 0;

    sum += sum_be16(&src.octets());
    sum += sum_be16(&dst.octets());
    sum += sum_be16(&len_bytes);
    sum += sum_be16(&nh_bytes);
    sum += sum_be16(icmp.get(0..2).unwrap_or_default());
    sum += sum_be16(icmp.get(4..).unwrap_or_default());

    while (sum >> 16) != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }

    !(sum as u16)
}
