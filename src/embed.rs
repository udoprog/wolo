use core::fmt;

use std::borrow::Cow;

use rust_embed::{EmbeddedFile, RustEmbed};
use serde::Serialize;

pub(crate) struct Base64([u8; 32]);

impl fmt::Display for Base64 {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use base64::display::Base64Display;
        use base64::engine::general_purpose::STANDARD;
        Base64Display::new(&self.0, &STANDARD).fmt(f)
    }
}

impl Serialize for Base64 {
    #[inline]
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.collect_str(self)
    }
}

#[derive(RustEmbed)]
#[folder = "static/"]
struct Assets;

pub(super) fn hash() -> Base64 {
    const FILES: &[&str] = &["style.css", "network.js"];

    let mut base = [0u8; 32];

    for path in FILES {
        let Some(style) = Assets::get(path) else {
            return Base64([0u8; 32]);
        };

        for (o, i) in base.iter_mut().zip(style.metadata.sha256_hash()) {
            *o ^= i;
        }
    }

    Base64(base)
}

pub(super) fn get(path: &str) -> Option<EmbeddedFile> {
    Assets::get(path)
}

pub(super) fn iter() -> impl Iterator<Item = Cow<'static, str>> {
    Assets::iter()
}
