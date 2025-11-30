//! Simple network monitor capable of sending magic Wake-on-LAN packets.
//!
//! Populate `/etc/ethers` (see `man ethers`) and run with:
//!
//! ```sh
//! wolo --bind 0.0.0.0:3000 --home home.md
//! ```
//!
//! The `home.md` file is expected to contain entries like these:
//!
//! ```md
//! # Default Title
//!
//! This is the landing page for your wolo installation. Please edit it by copying
//! it from the README.md and specify an alternative path using the --home option.
//!
//! * Network: /network
//! * Github: https://github.com/udoprog/wolo
//! ```
//!
//! This will populate a landing page at whatever port wolo is listening to.
//!
//! ![home](home.png)
//!
//! The `/network` page show an overview of the state of hosts on the network
//! and the ability to wake them up:
//!
//! ![showcase](showcase.png)
//!
//! <br>
//!
//! ## Options
//!
//! You can configure wolo with the following CLI options:
//! * Multiple `--ethers` arguments can be added to load `/etc/ethers` entries from
//!   multiple files. By default this is just set to `/etc/ethers`.

#![allow(clippy::drain_collect)]

use core::pin::pin;

use std::env;
use std::io::Cursor;
use std::os::fd::FromRawFd;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{Context, anyhow};
use axum::Router;
use axum::extract::State;
use axum::http::{StatusCode, Uri, header};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use clap::Parser;
use serde::Serialize;
use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::net::TcpListener;
use tokio::task;

use crate::embed::Base64;
use crate::utils::Templates;

mod embed;
mod host_name_cache;
mod hosts;
mod network;
mod ping_loop;
mod showcase;
mod utils;
mod wake_on_lan;

/// Path to load links from.
#[derive(Clone)]
pub struct S {
    home: Option<Arc<Path>>,
    templates: Templates,
}

pub struct StaticFile(Uri);

impl IntoResponse for StaticFile {
    fn into_response(self) -> Response {
        let path = self.0.path().trim_start_matches('/');

        match embed::Assets::get(path) {
            Some(content) => {
                let mime = mime_guess::from_path(path).first_or_octet_stream();
                ([(header::CONTENT_TYPE, mime.as_ref())], content.data).into_response()
            }
            None => (StatusCode::NOT_FOUND, "404 Not Found").into_response(),
        }
    }
}

#[derive(Parser)]
struct Opts {
    /// Address and port to bind the server to.
    #[clap(long, default_value = "0.0.0.0:3000")]
    bind: String,
    /// Path to load an ethers file from. By default this is `/etc/ethers`.
    ///
    /// The files specified in here will be monitored for changes and reloaded
    /// if needed.
    #[clap(long, default_value = "/etc/ethers")]
    ethers: Vec<PathBuf>,
    /// Path to load links from.
    #[clap(long)]
    home: Option<PathBuf>,
    /// Replaces real hostnames, macs, and ips with fake ones for demonstration.
    #[clap(long)]
    showcase: bool,
}

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt::init();

    if let Err(err) = inner().await {
        tracing::error!("Error: {err}");

        for e in err.chain().skip(1) {
            tracing::error!("Caused by: {e}");
        }

        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}

async fn inner() -> Result<(), anyhow::Error> {
    tracing::info!("prepare server...");

    let templates = crate::utils::load_templates().context("templates")?;

    let opts = Opts::try_parse()?;

    let showcase = showcase::new(opts.showcase);

    let mut hosts = hosts::State::builder();

    for path in &opts.ethers {
        hosts.add_ethers_path(path);
    }

    let hosts = hosts.build();
    let hosts_handle = tokio::spawn(hosts::spawn(hosts.clone()));

    let ping_state = ping_loop::State::new();
    let pinger_handle = task::spawn(ping_loop::new(ping_state.clone(), hosts.clone()));

    let state = S {
        home: opts.home.map(Arc::from),
        templates: templates.clone(),
    };

    // build our application with a route
    let app = Router::new()
        .route("/", get(root))
        .with_state(state)
        .nest(
            "/network",
            network::router(ping_state, "/network", templates, hosts.clone(), showcase),
        )
        .fallback(get(static_handler));

    tracing::info!("starting server...");

    let listener = if let Some(listener) =
        try_listener_from_env("LISTEN_FDS").context("setting up listen fd")?
    {
        tracing::info!("received socket through LISTEN_FDS");
        listener
    } else {
        let listener = TcpListener::bind(&opts.bind)
            .await
            .context("binding to address")?;

        let addr = listener.local_addr()?;
        tracing::info!("listening on http://{addr}");
        listener
    };

    tokio::select! {
        result = pinger_handle => {
            result?.context("pinger")?;
            tracing::info!("pinger task exited");
        }
        result = hosts_handle => {
            result.context("hosts")?;
            tracing::info!("hosts task exited");
        }
        result = axum::serve(listener, app) => {
            result.context("server")?;
            tracing::warn!("server exited");
        }
    }

    Ok(())
}

#[cfg(not(unix))]
fn try_listen_fds() -> Result<Option<TcpListener>, anyhow::Error> {
    Ok(None)
}

#[cfg(unix)]
fn try_listener_from_env(env: &'static str) -> Result<Option<TcpListener>, anyhow::Error> {
    let Ok(listen_fds) = env::var(env) else {
        return Ok(None);
    };

    let listen_fd: i32 = listen_fds.parse().with_context(|| anyhow!("parse {env}"))?;

    if listen_fd < 1 {
        return Ok(None);
    }

    let listener = unsafe { std::net::TcpListener::from_raw_fd(listen_fd) };
    listener.set_nonblocking(true).context("set nonblocking")?;
    let listener = TcpListener::from_std(listener).context("converting to tcp listener")?;
    Ok(Some(listener))
}

// Make our own error that wraps `anyhow::Error`.
struct Error(anyhow::Error);

impl<E> From<E> for Error
where
    E: Into<anyhow::Error>,
{
    fn from(err: E) -> Self {
        Self(err.into())
    }
}

// Tell axum how to convert `Error` into a response.
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Something went wrong: {}", self.0),
        )
            .into_response()
    }
}

// basic handler that responds with a static string
async fn root(
    State(S {
        home: path,
        templates,
        ..
    }): State<S>,
) -> Result<Html<String>, Error> {
    #[derive(Serialize)]
    struct Link {
        title: String,
        href: String,
    }

    fn parse_link(line: &str) -> Option<Link> {
        let (title, href) = line.split_once(':')?;

        Some(Link {
            title: title.trim().to_owned(),
            href: href.trim().to_owned(),
        })
    }

    #[derive(Serialize)]
    struct HomePage {
        hash: Base64,
        title: String,
        text: String,
        links: Vec<Link>,
    }

    impl HomePage {
        fn new() -> Self {
            Self {
                hash: embed::hash(),
                title: String::new(),
                text: String::new(),
                links: Vec::new(),
            }
        }

        async fn populate(&mut self, reader: impl AsyncRead) -> Result<(), Error> {
            let reader = pin!(BufReader::new(reader));
            let mut lines = reader.lines();

            while let Some(line) = lines.next_line().await? {
                if let Some(title) = line.trim_start().strip_prefix('#') {
                    self.title = title.trim().to_owned();
                    continue;
                }

                if let Some(line) = line.trim_start().strip_prefix('*') {
                    let Some(link) = parse_link(line) else {
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

            Ok(())
        }
    }

    let mut home = HomePage::new();

    if let Some(path) = path.as_deref()
        && let Ok(file) = File::open(path).await
    {
        home.populate(file).await?;
    } else if let Some(asset) = embed::Assets::get("home.md") {
        home.populate(Cursor::new(asset.data.as_ref())).await?;
    } else {
        home.title = "No Title".to_owned();
    }

    let o = templates.render("home.html", &home)?;
    Ok(Html(o))
}

async fn static_handler(uri: Uri) -> impl IntoResponse {
    StaticFile(uri)
}
