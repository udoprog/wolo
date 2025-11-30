use core::fmt;
use core::net::IpAddr;
use core::time::Duration;

use std::sync::Arc;
use std::time::Instant;

use axum::Router;
use axum::extract::{OriginalUri, Query, State};
use axum::http::uri::Builder;
use axum::response::{Html, Redirect};
use axum::routing::{get, post};
use axum_extra::extract::Form;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::embed::Base64;
use crate::hosts;
use crate::ping_loop;
use crate::showcase;
use crate::utils::Templates;
use crate::wake_on_lan::MagicPacket;
use crate::{Error, home};

#[derive(Clone)]
struct S {
    ping_state: ping_loop::State,
    prefix: &'static str,
    templates: Templates,
    hosts: hosts::State,
    showcase: showcase::Helper,
    home: Arc<home::HomePage>,
}

pub(super) async fn router(
    ping_state: ping_loop::State,
    prefix: &'static str,
    templates: Templates,
    hosts: hosts::State,
    showcase: showcase::Helper,
    home: home::Home,
) -> Router {
    let home = Arc::new(home.build().await);

    Router::new()
        .route("/", get(entry))
        .route("/wake", post(wake))
        .with_state(S {
            ping_state,
            prefix,
            templates,
            hosts,
            showcase,
            home,
        })
}

#[derive(Deserialize)]
struct Network {
    #[serde(default)]
    woke: Option<Uuid>,
    #[serde(default)]
    error: Option<String>,
}

// basic handler that responds with a static string
async fn entry(
    State(S {
        prefix,
        ping_state,
        templates,
        hosts,
        showcase,
        home,
        ..
    }): State<S>,
    Query(query): Query<Network>,
) -> Result<Html<String>, Error> {
    #[derive(Serialize)]
    struct PingError {
        error: String,
        ping: Option<String>,
        age: String,
    }

    #[derive(Serialize)]
    struct PingResult {
        class: &'static str,
        kind: String,
        outcome: String,
        code: Option<String>,
        target: IpAddr,
        source: IpAddr,
        dest: IpAddr,
        rtt: String,
        age: String,
        checksum: u16,
        expected_checksum: u16,
    }

    #[derive(Serialize)]
    struct Pending {
        errors: Vec<PingError>,
        results: Vec<PingResult>,
    }

    #[derive(Serialize)]
    struct Host {
        id: Uuid,
        just_woke: bool,
        names: Vec<String>,
        mac: Vec<String>,
        pending: Option<Pending>,
    }

    #[derive(Serialize)]
    struct Context {
        hash: Base64,
        title: String,
        prefix: &'static str,
        hosts: Vec<Host>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<&'static str>,
    }

    let mut showcase = showcase.lock().await;

    let hosts = hosts.hosts().await;
    let pinged = ping_state.pinged.lock().await;

    let mut context = Context {
        hash: crate::embed::hash(),
        title: home.title.clone().into_owned(),
        prefix,
        hosts: Vec::new(),
        error: match query.error.as_deref() {
            Some("unknown-host") => Some("Unknown host specified"),
            _ => None,
        },
    };

    let now = Instant::now();

    for host in hosts.iter() {
        let pending = match pinged.get(&host.id) {
            Some(pending) => {
                let mut errors = Vec::with_capacity(pending.errors.len());

                for e in &pending.errors {
                    errors.push(PingError {
                        error: e.error.clone(),
                        ping: e.ping.map(|p| p.to_string()),
                        age: duration(now.duration_since(e.sampled)).to_string(),
                    });
                }

                let mut results = Vec::with_capacity(pending.results.len());

                for r in &pending.results {
                    let code = match r.outcome {
                        lib::Outcome::V4(lib::icmp::v4::Type::UNREACHABLE) => {
                            let code = lib::icmp::v4::UnreachableCode::new(r.code);
                            Some(code.to_string())
                        }
                        lib::Outcome::V6(lib::icmp::v6::Type::UNREACHABLE) => {
                            let code = lib::icmp::v6::Unreachable::new(r.code);
                            Some(code.to_string())
                        }
                        _ => {
                            if r.code != 0 {
                                Some(r.code.to_string())
                            } else {
                                None
                            }
                        }
                    };

                    results.push(PingResult {
                        class: if r.outcome.is_echo_reply() {
                            "success"
                        } else {
                            "error"
                        },
                        kind: r.kind.to_string(),
                        outcome: r.outcome.to_string(),
                        code,
                        target: showcase.ip(r.target),
                        source: showcase.ip(r.source),
                        dest: showcase.ip(r.dest),
                        rtt: duration(r.rtt).to_string(),
                        age: duration(now.duration_since(r.sampled)).to_string(),
                        checksum: r.checksum,
                        expected_checksum: r.expected_checksum,
                    });
                }

                Some(Pending { errors, results })
            }
            None => None,
        };

        let just_woke = query.woke.map(|id| id == host.id).unwrap_or_default();

        context.hosts.push(Host {
            id: host.id,
            just_woke,
            names: host
                .names()
                .map(|n| showcase.host_name(host.id, n))
                .collect(),
            mac: host
                .macs
                .iter()
                .map(|m| showcase.mac(*m).to_string())
                .collect(),
            pending,
        });
    }

    let o = templates.render("network.html", context)?;
    Ok(Html(o))
}

fn duration(d: Duration) -> impl fmt::Display {
    struct D(Duration);

    impl fmt::Display for D {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            let secs = self.0.as_secs();

            if secs > 86400 {
                return write!(f, "{} d", secs / 86400);
            }

            if secs > 3600 {
                return write!(f, "{} h", secs / 3600);
            }

            if secs > 60 {
                write!(f, "{} m", secs / 60)?;
                return Ok(());
            }

            let nanos = self.0.subsec_nanos();

            let millis = nanos / 1_000_000;

            if millis > 0 {
                return write!(f, "{secs}.{millis:03} s");
            }

            let micros = nanos / 1_000;

            if micros > 0 {
                return write!(f, "{millis}.{micros:03} ms");
            }

            write!(f, "{secs}.{nanos:03} µs")
        }
    }

    D(d)
}

#[derive(Deserialize)]
struct Wake {
    host: Uuid,
}

async fn wake(
    State(S { prefix, hosts, .. }): State<S>,
    OriginalUri(uri): OriginalUri,
    Form(wake): Form<Wake>,
) -> Result<Redirect, Error> {
    let hosts = hosts.hosts().await;

    let Some(host) = hosts.iter().find(|h| h.id == wake.host) else {
        let redirect = format!("{uri}?error=unknown-host");
        let redirect = Redirect::to(&redirect);
        return Ok(redirect);
    };

    let builder = Builder::from(uri).path_and_query(format!("{prefix}?woke={}", host.id));
    let uri = builder.build()?;

    for mac in &host.macs {
        let packet = MagicPacket::new(mac.into_array());
        packet.send().await?;
    }

    let redirect = format!("{uri}#host-{}", host.id);
    let redirect = Redirect::to(&redirect);
    Ok(redirect)
}
