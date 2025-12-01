#![allow(clippy::new_without_default)]

mod error;
pub use self::error::Error;

mod pinger;
pub use self::pinger::{Outcome, Pinger, Response};

mod buf;
pub use self::buf::Buffer;

pub mod icmp;
mod ip;
