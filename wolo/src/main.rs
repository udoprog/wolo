//! Simple network monitor capable of sending magic Wake-on-LAN packets.
//!
//! Populate `/etc/ethers` (`man ethers`) and/or `/etc/hosts` (`man hosts`) and
//! run with:
//!
//! ```sh
//! wolo --bind 0.0.0.0:3000 --home home.md
//! ```
//!
//! The `home.md` is used to populate the landing page, see [Landing
//! Page](#landing-page) below for how to configure this.
//!
//! ![home](home.png)
//!
//! The `/network` page show an overview of the state of hosts on the network
//! and the ability to wake them up if they have configured mac addresses.
//!
//! ![showcase](showcase.png)
//!
//! <br>
//!
//! ## Configuration
//!
//! The wolo service can take configuration from multiple sources:
//!
//! * By default we parse `/etc/hosts` to find hosts to interact with.
//!   Additional hosts files can be specified using `--hosts <path>`.
//! * By default we parse `/etc/ethers` to find and associate hosts with MAC
//!   addresses. Additional files of this format can be specified using
//!   `--ethers <path>`.
//! * Any number of optional configuration files can be specified using
//!   `--config <path>`.
//!
//! The configuration files are in toml, and have the following format:
//!
//! ```toml
//! # The default socket address to bind to.
//! # Can be IPv4 or IPv6.
//! bind = "0.0.0.0:3000"
//!
//! # Simple variant of a list of hosts.
//! hosts = ["example.com", "another.example.com"]
//!
//! # Detailed host configuration.
//! [hosts."example.com"]
//! # Collection of mac addresses associated with this host.
//! macs = ["00:11:22:33:44:55"]
//! # Setting the preferred name will make it so that only this name is
//! # displayed in the network view for this host.
//! preferred_name = "example"
//! # Whether this host should be ignored.
//! #
//! # Additional hosts to be ignored can be specified with the
//! # `--ignore-host` option.
//! ignore = false
//! ```
//!
//! <br>
//!
//! #### Landing Page
//!
//! We expect a landing page to be specified in markdown either through the
//! `home` option or the `--home` cli option. This can be dynamically changed
//! while the service is running.
//!
//! ```md
//! # wolo
//!
//! This is the landing page for your wolo installation. Please edit it by copying
//! it from the README.md and specify an alternative path using the --home option.
//!
//! * [Network](/network)
//! * [Github](https://github.com/udoprog/wolo)
//! ```
//!
//! Note that arbitrary markdown is not supported. Only the given structures are
//! supported. The first title, paragraphs and links in list will simply be
//! extracted and used to build the landing page. Warnings will be emitted for
//! entries which are currently skipped.

#![allow(clippy::drain_collect)]

use std::env;
use std::os::fd::FromRawFd;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use axum::Router;
use axum::extract::State;
use axum::http::{StatusCode, Uri, header};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use clap::Parser;
use tokio::net::TcpListener;
use tokio::task;

use crate::utils::Templates;

mod config;
mod embed;
mod home;
mod host_name_cache;
mod hosts;
mod network;
mod ping_loop;
mod showcase;
mod utils;
mod wake_on_lan;

const DEFAULT_BIND: &str = "0.0.0.0:3000";

/// Path to load links from.
#[derive(Clone)]
pub struct S {
    home: home::Home,
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
    /// Path to load configuration files from.
    #[clap(long, default_value = "/etc/wolo/config.toml")]
    config: Vec<PathBuf>,
    /// Address and port to bind the server to. Defaults to `0.0.0.0:3000`.
    #[clap(long)]
    bind: Option<String>,
    /// Path to load landing page configuration from.
    #[clap(long)]
    home: Option<PathBuf>,
    /// Path to load an ethers file from. By default this is `/etc/ethers`.
    ///
    /// The files specified in here will be monitored for changes and reloaded
    /// if needed.
    #[clap(long, default_value = "/etc/ethers")]
    ethers: Vec<PathBuf>,
    /// Path to load hosts files from. By default this is `/etc/hosts`.
    ///
    /// The files specified in here will be monitored for changes and reloaded
    /// if needed.
    #[clap(long, default_value = "/etc/hosts")]
    hosts: Vec<PathBuf>,
    /// Specify hosts to ignore.
    ///
    /// This will ensure that the host is ignored even if it's part of
    /// configuration.
    #[clap(long)]
    ignore_host: Vec<String>,
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

async fn inner() -> Result<()> {
    let templates = crate::utils::load_templates().context("templates")?;

    let opts = Opts::try_parse()?;

    let mut config = config::Config::default();

    let mut has_errors = false;

    for path in &opts.config {
        let d = config::Diagnostics::new();

        config
            .add_from_path(path, &d)
            .with_context(|| path.display().to_string())?;

        for error in d.into_errors() {
            tracing::error!("{}: {error}", path.display());
            has_errors = true;
        }
    }

    for host in &opts.ignore_host {
        config.ignore_host(host);
    }

    if has_errors {
        return Err(anyhow!("Configuration had errors"));
    }

    let config = Arc::new(config);

    let showcase = showcase::new(opts.showcase);

    let mut hosts = hosts::State::builder();

    for path in &opts.ethers {
        hosts.add_ethers_path(path);
    }

    for path in &opts.hosts {
        hosts.add_hosts_path(path);
    }

    let home = home::new(opts.home.as_deref());

    let hosts = hosts.build();
    let hosts_handle = tokio::spawn(hosts::spawn(hosts.clone(), config.clone()));

    let ping_state = ping_loop::State::new();
    let pinger_handle = task::spawn(ping_loop::new(ping_state.clone(), hosts.clone()));

    let state = S {
        home: home.clone(),
        templates: templates.clone(),
    };

    let network = network::router(
        ping_state,
        "/network",
        templates,
        hosts.clone(),
        showcase,
        home,
    )
    .await?;

    // build our application with a route
    let app = Router::new()
        .route("/", get(root))
        .with_state(state)
        .nest("/network", network)
        .fallback(get(static_handler));

    let bind = opts
        .bind
        .as_deref()
        .or(config.bind.as_deref())
        .unwrap_or(DEFAULT_BIND);

    let listener = if let Some(listener) =
        try_listener_from_env("LISTEN_FDS").context("setting up listen fd")?
    {
        tracing::info!("received socket through LISTEN_FDS");
        listener
    } else {
        let listener = TcpListener::bind(&bind)
            .await
            .context("binding to address")?;

        let addr = listener.local_addr()?;
        tracing::info!("Listening on http://{addr}");
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
fn try_listen_fds() -> Result<Option<TcpListener>> {
    Ok(None)
}

#[cfg(unix)]
fn try_listener_from_env(env: &'static str) -> Result<Option<TcpListener>> {
    let Ok(listen_fds) = env::var(env) else {
        return Ok(None);
    };

    let listen_fd: i32 = listen_fds.parse().with_context(|| anyhow!("parse {env}"))?;

    if listen_fd < 1 {
        return Ok(None);
    }

    // NB: This is currently broken since what's passed in is a single connected
    // peer, not a listening socket.
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
        home, templates, ..
    }): State<S>,
) -> Result<Html<String>, Error> {
    let home = home.build().await;
    let o = templates.render("home.html", &home)?;
    Ok(Html(o))
}

async fn static_handler(uri: Uri) -> impl IntoResponse {
    StaticFile(uri)
}
