use core::pin::pin;
use std::borrow::Cow;
use std::io::Cursor;
use std::path::Path;
use std::sync::Arc;

use serde::Serialize;
use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};

use crate::embed;
use crate::embed::Base64;

/// Construct a new home handle.
pub fn new(home_path: Option<&Path>) -> Home {
    Home {
        home_path: home_path.map(Arc::from),
    }
}

#[derive(Clone)]
pub struct Home {
    home_path: Option<Arc<Path>>,
}

#[derive(Serialize)]
struct Link {
    title: String,
    href: String,
}

fn parse_link(line: &str) -> Option<Link> {
    let at = line.find('(')?;

    let (title, href) = line.split_at_checked(at)?;

    let title = title.trim_start_matches('[').trim_end_matches(']');
    let href = href.trim_start_matches('(').trim_end_matches(')');

    Some(Link {
        title: title.trim().to_owned(),
        href: href.trim().to_owned(),
    })
}

impl Home {
    /// Build a home page from the configured path or embedded asset.
    pub async fn build(&self) -> HomePage {
        let mut home = HomePage::new();

        if let Some(path) = self.home_path.as_deref()
            && let Ok(file) = File::open(path).await
        {
            home.populate(file).await;
            return home;
        }

        if let Some(asset) = embed::get("home.md") {
            home.populate(Cursor::new(asset.data.as_ref())).await;
        }

        home
    }
}

/// The state associated with the home page.
#[derive(Serialize)]
pub struct HomePage {
    hash: Base64,
    pub title: Cow<'static, str>,
    text: String,
    links: Vec<Link>,
}

impl HomePage {
    /// Construct a new home page builder.
    pub fn new() -> Self {
        Self {
            hash: crate::embed::hash(),
            title: Cow::Borrowed("wolo"),
            text: String::new(),
            links: Vec::new(),
        }
    }

    /// Populate the home page from an asynchronous reader.
    async fn populate(&mut self, reader: impl AsyncRead) {
        let reader = pin!(BufReader::new(reader));
        let mut lines = reader.lines();

        while let Ok(Some(line)) = lines.next_line().await {
            if let Some(title) = line.trim_start().strip_prefix('#') {
                self.title = Cow::Owned(title.trim().to_owned());
                continue;
            }

            if let Some(line) = line.trim_start().strip_prefix('*') {
                let Some(link) = parse_link(line.trim()) else {
                    continue;
                };

                self.links.push(link);
                continue;
            }

            let line = line.trim();

            if !line.is_empty() {
                self.text.push_str(line);
                self.text.push('\n');
            }
        }
    }
}
