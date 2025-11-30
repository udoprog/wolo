use core::fmt;
use core::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use core::pin::pin;
use core::time::Duration;

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Error};
use lib::{Buffer, Outcome, Pinger};
use tokio::sync::Mutex;
use tokio::time;
use uuid::Uuid;

use crate::host_name_cache::{CacheNameResult, HostNameCache, HostNameCacheLookup};
use crate::hosts;

const TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct PingResult {
    pub kind: PingKind,
    pub outcome: Outcome,
    pub code: u8,
    pub rtt: Duration,
    pub sampled: Instant,
    pub target: IpAddr,
    pub source: IpAddr,
    pub dest: IpAddr,
    pub checksum: u16,
    pub expected_checksum: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PingKind {
    V4,
    V6,
}

impl fmt::Display for PingKind {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PingKind::V4 => write!(f, "ICMPv4"),
            PingKind::V6 => write!(f, "ICMPv6"),
        }
    }
}

#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Pending {
    pub id: Uuid,
    pub errors: Vec<PingError>,
    pub results: Vec<PingResult>,
}

#[derive(Clone)]
pub struct State {
    /// Hosts that have been pinged.
    pub pinged: Arc<Mutex<HashMap<Uuid, Pending>>>,
}

impl State {
    /// Construct a new empty state.
    #[inline]
    pub fn new() -> Self {
        Self {
            pinged: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

#[derive(Debug, Clone)]
pub struct PingError {
    pub error: String,
    pub ping: Option<IpAddr>,
    pub sampled: Instant,
}

struct Task {
    id: Uuid,
    handle: HostNameCacheLookup,
}

struct Resolve {
    id: Uuid,
    addresses: BTreeSet<SocketAddr>,
    errors: Vec<PingError>,
}

impl Resolve {
    fn new(id: Uuid) -> Self {
        Self {
            id,
            addresses: BTreeSet::new(),
            errors: Vec::new(),
        }
    }
}

struct PingerService {
    v4: Pinger,
    v6: Pinger,
    b1: Buffer,
    b2: Buffer,
    seen: HashSet<Uuid>,
    waiting: HashMap<u64, (usize, Instant, IpAddr)>,
    pending: Vec<Pending>,
    id: u64,
    tasks: Vec<Task>,
    resolved: Vec<Resolve>,
}

impl PingerService {
    async fn setup_pings(&mut self) {
        for Resolve {
            id,
            addresses,
            mut errors,
        } in self.resolved.drain(..)
        {
            let index = self.pending.len();

            for address in addresses {
                let next = self.id.to_be_bytes();
                let started = Instant::now();

                match address {
                    SocketAddr::V4(addr) => {
                        let ip = addr.ip();

                        pub fn is_unicast(addr: &Ipv4Addr) -> bool {
                            !addr.is_multicast()
                                && !addr.is_loopback()
                                && !addr.is_link_local()
                                && !addr.is_broadcast()
                                && !addr.is_documentation()
                                && !addr.is_unspecified()
                        }

                        if !is_unicast(ip) {
                            continue;
                        }

                        if let Err(error) = self.v4.ping(&mut self.b1, IpAddr::V4(*ip), &next).await
                        {
                            errors.push(PingError {
                                error: format!("{error}"),
                                ping: Some(address.ip()),
                                sampled: started,
                            });
                        } else {
                            self.waiting.insert(self.id, (index, started, address.ip()));
                        }
                    }
                    SocketAddr::V6(addr) => {
                        let ip = addr.ip();

                        pub fn is_unicast(addr: &Ipv6Addr) -> bool {
                            !addr.is_multicast()
                                && !addr.is_loopback()
                                && !addr.is_unicast_link_local()
                                && !addr.is_unspecified()
                        }

                        if !is_unicast(ip) {
                            continue;
                        }

                        if let Err(error) = self.v6.ping(&mut self.b2, IpAddr::V6(*ip), &next).await
                        {
                            errors.push(PingError {
                                error: format!("{error}"),
                                ping: Some(address.ip()),
                                sampled: started,
                            });
                        } else {
                            self.waiting.insert(self.id, (index, started, address.ip()));
                        }
                    }
                }

                self.id = self.id.wrapping_add(1);
            }

            self.pending.push(Pending {
                id,
                errors,
                results: Vec::new(),
            });
        }
    }

    async fn wait_for_result(&mut self) -> Result<(), Error> {
        let (r, kind, b) = tokio::select! {
            r = self.v4.recv(&mut self.b1) => {
                (r?, PingKind::V4, &self.b1)
            }
            r = self.v6.recv(&mut self.b2) => {
                (r?, PingKind::V6, &self.b2)
            }
        };

        let bytes = b.read::<[u8; 8]>().context("reading response payload")?;

        let id = u64::from_be_bytes(*bytes);

        if let Some((index, started, target)) = self.waiting.remove(&id) {
            let rtt = started.elapsed();

            if let Some(pending) = self.pending.get_mut(index) {
                pending.results.push(PingResult {
                    kind,
                    outcome: r.outcome,
                    code: r.code,
                    rtt,
                    sampled: Instant::now(),
                    target,
                    source: r.source,
                    dest: r.dest,
                    checksum: r.checksum,
                    expected_checksum: r.expected_checksum,
                });
            }
        }

        Ok(())
    }
}

pub(super) async fn new(state: State, hosts: hosts::State) -> Result<(), Error> {
    let mut s = PingerService {
        v4: Pinger::v4()?,
        v6: Pinger::v6()?,
        b1: Buffer::new(),
        b2: Buffer::new(),
        seen: HashSet::new(),
        waiting: HashMap::new(),
        pending: Vec::new(),
        id: 0u64,
        tasks: Vec::new(),
        resolved: Vec::new(),
    };

    let mut cache = HostNameCache::new();

    loop {
        s.seen.clear();

        cache.evict_old().await;

        for host in hosts.hosts().await.iter() {
            if !s.seen.insert(host.id) {
                continue;
            }

            s.tasks.push(Task {
                id: host.id,
                handle: cache.get(host).await,
            });
        }

        for Task { id, handle } in s.tasks.drain(..) {
            let mut resolve = Resolve::new(id);

            let results = match handle.get().await {
                Ok(results) => results,
                Err(error) => {
                    resolve.errors.push(PingError {
                        error: error.to_string(),
                        ping: None,
                        sampled: Instant::now(),
                    });

                    s.resolved.push(resolve);
                    continue;
                }
            };

            let sampled = Instant::now();

            for result in results.iter() {
                match result {
                    CacheNameResult::Address(addr) => {
                        resolve.addresses.insert(*addr);
                    }
                    CacheNameResult::Error(error) => {
                        resolve.errors.push(PingError {
                            error: format!("{}: {}", error.name, error.error),
                            ping: None,
                            sampled,
                        });
                    }
                }
            }

            s.resolved.push(resolve);
        }

        s.setup_pings().await;

        let mut timeout = pin!(time::sleep(TIMEOUT));

        while !s.waiting.is_empty() {
            tokio::select! {
                _ = timeout.as_mut() => {
                    break;
                }
                result = s.wait_for_result() => {
                    if let Err(error) = result {
                        tracing::error!("Receive error: {error}");
                        let mut source = error.source();

                        while let Some(err) = source {
                            tracing::error!("Caused by: {err}");
                            source = err.source();
                        }
                    }
                }
            }
        }

        let sampled = Instant::now();

        for (_, (index, _, addr)) in s.waiting.drain() {
            if let Some(p) = s.pending.get_mut(index) {
                p.errors.push(PingError {
                    error: "timeout".to_owned(),
                    ping: Some(addr),
                    sampled,
                });
            }
        }

        for p in &mut s.pending {
            p.results.sort_by_key(|r| (r.kind, r.source));
            p.errors.sort_by_key(|e| e.ping);
        }

        {
            let mut pinged = state.pinged.lock().await;
            pinged.clear();

            for p in s.pending.drain(..) {
                pinged.insert(p.id, p);
            }
        }

        timeout.await;
    }
}
