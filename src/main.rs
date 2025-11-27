use std::collections::BTreeMap;
use std::env;
use std::fmt::Write;
use std::os::fd::FromRawFd;
use std::process::ExitCode;

use anyhow::{Context, anyhow};
use axum::Router;
use axum::extract::{OriginalUri, Query};
use axum::http::StatusCode;
use axum::http::uri::Builder;
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum_extra::extract::Form;
use clap::Parser;
use macaddr::MacAddr6;
use serde::{Deserialize, de};
use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::TcpListener;

use crate::wake_on_lan::MagicPacket;

mod wake_on_lan;

const STYLE: &str = r#"
    body {
        font-family: Arial, sans-serif;
        margin: 2em;
        background-color: #f9f9f9;
    }

    h1, h4 {
        color: #333333;
        margin: 0;
        padding: 0;
    }

    form {
        text-align: center;
        padding: 0.5em;
        background-color: #ecececff;
        border: 1px solid #bbbbbbff;
        border-radius: 4px;
    }

    form > * {
        margin: 0.5em 0;
    }

    form > *:first-child {
        margin-top: 0;
    }

    form > *:last-child {
        margin-bottom: 0;
    }

    button {
        font-size: 150%;
        padding: 0.5em 1em;
        background-color: #4CAF50;
        color: white;
        border: none;
        border-radius: 4px;
        cursor: pointer;
    }

    button:hover {
        background-color: #45a049;
    }

    .mac {
        font-family: monospace;
    }

    .just-woke {
        color: #008000;
        font-weight: bold;
        font-size: 0.8em;
    }
"#;

#[derive(Parser)]
struct Opts {
    /// Address and port to  bind the server to
    #[clap(long, default_value = "0.0.0.0:3000")]
    bind: String,
}

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt::init();

    if let Err(err) = inner().await {
        tracing::error!("Error: {err}");
        eprintln!("Error: {err}");
        eprintln!("Backtrace: {}", err.backtrace());

        for e in err.chain().skip(1) {
            tracing::error!("Caused by: {e}");
            eprintln!("Caused by: {e}");
        }

        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}

async fn inner() -> Result<(), anyhow::Error> {
    tracing::info!("prepare server...");

    // build our application with a route
    let app = Router::new()
        .route("/", get(root))
        .route("/wake", post(wake));

    let opts = Opts::try_parse()?;

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

    axum::serve(listener, app).await?;
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

struct StringMacAddr6(MacAddr6);

impl<'de> Deserialize<'de> for StringMacAddr6 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;

        let mac = s.parse::<MacAddr6>().map_err(de::Error::custom)?;

        Ok(StringMacAddr6(mac))
    }
}

// Make our own error that wraps `anyhow::Error`.
struct Error(anyhow::Error);

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

// This enables using `?` on functions that return `Result<_, anyhow::Error>` to turn them into
// `Result<_, Error>`. That way you don't need to do that manually.
impl<E> From<E> for Error
where
    E: Into<anyhow::Error>,
{
    fn from(err: E) -> Self {
        Self(err.into())
    }
}

#[derive(Deserialize)]
struct Root {
    #[serde(default)]
    woke: Option<StringMacAddr6>,
}

// basic handler that responds with a static string
async fn root(Query(query): Query<Root>) -> Result<Html<String>, Error> {
    let f = File::open("/etc/ethers").await?;
    let reader = BufReader::new(f);
    let mut o = String::new();

    let just_woke = query.woke.map(|m| m.0);

    let mut seen = BTreeMap::<MacAddr6, Vec<String>>::new();

    write!(o, "<html>")?;
    write!(o, "<head>")?;

    write!(o, "<style>{STYLE}</style>")?;

    write!(o, "</head>")?;

    write!(o, "<body>")?;
    write!(o, "<h1>Click host to Wake-on-LAN</h1>")?;
    write!(
        o,
        "<p>This is based off the <b>/etc/ethers</b> file of the host running this program</p>"
    )?;

    let mut lines = reader.lines();

    while let Some(line) = lines.next_line().await? {
        let line = line.trim();

        let Some((mac, name)) = line.split_once(' ') else {
            continue;
        };

        let Ok(mac) = mac.parse::<MacAddr6>() else {
            continue;
        };

        seen.entry(mac).or_default().push(name.to_owned());
    }

    for (mac, names) in seen.iter() {
        write!(o, "<form action=\"/wake\" method=\"post\">")?;

        let names = names.join(", ");

        write!(o, "<h4>💻 {names}</h4>")?;

        write!(o, "<div class=\"mac\">MAC: {mac}</div>")?;

        if Some(*mac) == just_woke {
            write!(o, "<div class=\"just-woke\">Magic Packet Sent</div>")?;
        }

        write!(
            o,
            "<button type=\"submit\" name=\"mac\" value=\"{mac}\">Wake</button>"
        )?;

        write!(o, "</form>")?;
    }

    write!(o, "</body>")?;
    write!(o, "</html>")?;
    Ok(Html(o))
}

#[derive(Deserialize)]
struct Wake {
    mac: StringMacAddr6,
}

async fn wake(OriginalUri(uri): OriginalUri, Form(wake): Form<Wake>) -> Result<Redirect, Error> {
    let builder = Builder::from(uri).path_and_query(format!("/?woke={}", wake.mac.0));
    let uri = builder.build()?;

    let packet = MagicPacket::new(wake.mac.0.into_array());
    packet.send().await?;

    let redirect = format!("{uri}");
    let redirect = Redirect::to(&redirect);
    Ok(redirect)
}
