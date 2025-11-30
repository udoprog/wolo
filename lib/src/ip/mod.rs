pub(crate) mod v4 {
    use core::ffi::c_int;

    use crate::buf::Aligned;

    unsafe impl Aligned for Header {}

    /// The IPv4 header structure.
    #[derive(Debug, Clone, Copy)]
    #[repr(C)]
    pub struct Header {
        version: u8,
        _r0: [u8; 8],
        protocol: u8,
        _r1: [u8; 10],
    }

    impl Header {
        /// Get the version from the header.
        pub fn version(&self) -> u8 {
            self.version >> 4
        }

        /// Get the protocol from the header.
        pub fn protocol(&self) -> c_int {
            self.protocol as c_int
        }
    }
}
