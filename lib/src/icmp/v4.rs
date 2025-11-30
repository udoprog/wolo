use core::fmt;
use core::mem::{MaybeUninit, size_of};
use core::slice;

use crate::buf::Aligned;

/// The type of an ICMP message.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct Type(u8);

super::macros::define_types! {
    impl Type {
        /// echo reply
        ECHO_REPLY = 0;
        /// destination host unreachable
        UNREACHABLE = 3;
        /// echo request
        ECHO_REQUEST = 8;
    }
}

/// The code for a Destination Unreachable ICMP message.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct UnreachableCode(u8);

super::macros::define_types! {
    impl UnreachableCode {
        /// net unreachable
        NET_UNREACHABLE = 0;
        /// host unreachable
        HOST_UNREACHABLE = 1;
        /// protocol unreachable
        PROTOCOL_UNREACHABLE = 2;
        /// port unreachable
        PORT_UNREACHABLE = 3;
        /// fragmentation needed and don't fragment was set
        FRAGMENTATION_NEEDED = 4;
        /// source route failed
        SOURCE_ROUTE_FAILED = 5;
        /// destination network unknown
        DESTINATION_NETWORK_UNKNOWN = 6;
        /// destination host unknown
        DESTINATION_HOST_UNKNOWN = 7;
        /// source host isolated
        SOURCE_HOST_ISOLATED = 8;
        /// communication with destination network is administratively prohibited
        NETWORK_ADMINISTRATIVELY_PROHIBITED = 9;
        /// communication with destination host is administratively prohibited
        HOST_ADMINISTRATIVELY_PROHIBITED = 10;
        /// destination network unreachable for type of service
        NETWORK_UNREACHABLE_SERVICE = 11;
        /// destination host unreachable for type of service
        HOST_UNREACHABLE_SERVICE = 12;
        /// communication administratively prohibited
        ADMINISTRATIVELY_PROHIBITED = 13;
        /// host precedence violation
        HOST_PRECEDENCE_VIOLATION = 14;
        /// precedence cutoff in effect
        PRECEDENCE_CUTOFF_IN_EFFECT = 15;
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

    /// Set the checksum in the header.
    #[inline]
    pub fn set_checksum(&mut self, checksum: u16) {
        self.checksum = checksum.to_be();
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

    for c in chunks.by_ref() {
        let &[a, b] = c else {
            continue;
        };

        let word = u16::from_be_bytes([a, b]);
        sum += word as u64;
    }

    if let &[last] = chunks.remainder() {
        let word = u16::from_be_bytes([last, 0]);
        sum += word as u64;
    }

    sum
}

pub fn checksum(icmp: &[u8]) -> u16 {
    let mut sum: u64 = 0;

    sum += sum_be16(icmp.get(0..2).unwrap_or_default());
    sum += sum_be16(icmp.get(4..).unwrap_or_default());

    while (sum >> 16) != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }

    !(sum as u16)
}
