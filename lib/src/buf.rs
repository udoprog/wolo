use core::cell::Cell;
use core::fmt;
use core::mem::{MaybeUninit, size_of};
use core::slice;

use crate::error::{Error, ErrorKind};

const MTU: usize = 1500;

/// Implement a type that can be accessed directly out of a buffer.
///
/// # Safety
///
/// Implementers guarantee that:
/// * The type is aligned to 2 bytes.
/// * The type implementing it is `repr(C)`.
/// * The type can inhabit any bit patterns.
pub unsafe trait Aligned {}

unsafe impl<const N: usize> Aligned for [u8; N] {}

/// A buffer sized to the maximum transmission unit (MTU), aligned to 2 bytes to
/// allow header operations to be performed directly on the buffer.
#[repr(align(16))]
pub struct Buffer<const N: usize = MTU> {
    /// Source buffer.
    buf: [MaybeUninit<u8>; N],
    /// Position being read.
    at: Cell<usize>,
    /// Length that has been initialized by a read.
    init: usize,
}

impl Buffer {
    /// Create a new MTU-sized buffer.
    pub fn new() -> Self {
        Self {
            buf: [MaybeUninit::uninit(); MTU],
            at: Cell::new(0),
            init: 0,
        }
    }
}

impl<const N: usize> Buffer<N> {
    /// Clear the buffer.
    pub fn clear(&mut self) {
        self.at.set(0);
        self.init = 0;
    }

    /// Advance the initialized length of the buffer by the given amount.
    #[inline]
    pub fn advance(&mut self, len: usize) {
        self.init = self.init.saturating_add(len).min(N);
    }

    /// Get remaining number of initialized bytes in the buffer.
    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        unsafe {
            slice::from_raw_parts(
                self.buf.as_ptr().cast::<u8>().wrapping_add(self.at.get()),
                self.init.saturating_sub(self.at.get()),
            )
        }
    }

    /// Get remaining number of uninitialized bytes in the buffer.
    pub fn remaining_mut(&self) -> usize {
        N.saturating_sub(self.init)
    }

    /// Get a mutable uninitialized slice of the buffer.
    #[inline]
    pub fn as_uninit_mut(&mut self) -> &mut [MaybeUninit<u8>] {
        unsafe {
            slice::from_raw_parts_mut(
                self.buf.as_mut_ptr().wrapping_add(self.at.get()),
                self.remaining_mut(),
            )
        }
    }

    /// Read a value of type T from the start of the buffer.
    #[inline]
    pub fn read<T>(&self) -> Result<&T, Error>
    where
        T: Aligned,
    {
        const {
            assert!(align_of::<T>() <= 2, "Header must be aligned to 2 bytes");
            assert!(
                size_of::<T>().is_multiple_of(2),
                "Header size must be a multiple of 2 bytes"
            );
        }

        let size = size_of::<T>();
        let end = self.at.get().wrapping_add(size);

        if self.init < end {
            return Err(Error::new(ErrorKind::BufferTooSmall {
                actual: self.init,
                needed: end,
            }));
        }

        let ptr = self.buf.as_ptr().wrapping_add(self.at.get()).cast::<T>();
        self.at.set(end);
        unsafe { Ok(&*ptr) }
    }

    /// Extend the buffer by copying data from the given slice.
    pub fn extend_from_slice(&mut self, data: &[u8]) {
        let len = data.len().min(self.buf.len().saturating_sub(self.init));

        unsafe {
            self.buf
                .as_mut_ptr()
                .cast::<u8>()
                .wrapping_add(self.init)
                .copy_from_nonoverlapping(data.as_ptr(), len);
        }

        self.init += len;
    }
}

impl<const N: usize> fmt::Debug for Buffer<N> {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_bytes().fmt(f)
    }
}
