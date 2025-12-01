use core::fmt;
use core::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use core::pin::pin;
use core::time::Duration;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use anyhow::{Context, Error};
use lib::{Buffer, Outcome, Pinger, Response};
use tokio::sync::Mutex;
use tokio::task::JoinSet;
use tokio::time::{self, Instant};
use uuid::Uuid;

use crate::host_name_cache::{CacheNameResult, HostNameCache};
use crate::hosts;

const TIMEOUT: Duration = Duration::from_secs(10);
const NEXT: Duration = Duration::from_secs(1);

#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct PingResult {
    pub kind: PingKind,
    pub outcome: Outcome,
    pub code: u8,
    pub sequence: u16,
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

#[derive(Default, Debug, Clone)]
#[non_exhaustive]
pub struct Pinged {
    pub errors: Vec<PingError>,
    pub results: Vec<PingResult>,
}

impl Pinged {
    pub fn result(&mut self, result: PingResult) {
        self.errors
            .retain(|e| e.kind.as_address() != Some(result.target));

        if let Some(r) = self.results.iter_mut().find(|r| r.target == result.target) {
            *r = result;
            return;
        }

        self.results.push(result);
        self.results.sort_by_key(|r| r.target);
    }

    /// Add a ping error, replacing any existing error of the same kind.
    pub fn error(&mut self, error: PingError) {
        if let PingErrorKind::Address(addr) = error.kind {
            self.results.retain(|r| r.target != addr);
        }

        if let Some(e) = self.errors.iter_mut().find(|e| e.kind == error.kind) {
            *e = error;
            return;
        }

        self.errors.retain(|e| e.kind != error.kind);
        self.errors.push(error);
        self.errors.sort_by(|a, b| a.kind.cmp(&b.kind));
    }
}

#[derive(Clone)]
pub struct State {
    /// Hosts that have been pinged.
    pub pinged: Arc<Mutex<HashMap<Uuid, Pinged>>>,
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

/// The kind of ping error.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[non_exhaustive]
pub enum PingErrorKind {
    Address(IpAddr),
    Host(String),
}

impl PingErrorKind {
    /// Coerces to an address if possible.
    pub fn as_address(&self) -> Option<IpAddr> {
        match self {
            PingErrorKind::Address(addr) => Some(*addr),
            PingErrorKind::Host(_) => None,
        }
    }

    /// Coerces to a host name if possible.
    pub fn as_host(&self) -> Option<&str> {
        match self {
            PingErrorKind::Address(_) => None,
            PingErrorKind::Host(name) => Some(name),
        }
    }
}

#[derive(Debug, Clone)]
pub struct PingError {
    pub error: String,
    pub kind: PingErrorKind,
    pub sampled: Instant,
}

struct PingerService {
    v4: Pinger,
    v6: Pinger,
    b1: Buffer,
    b2: Buffer,
    id: u64,
}

impl PingerService {
    async fn ping(&mut self, address: IpAddr) -> Result<Option<u64>, Error> {
        match address {
            IpAddr::V4(ip) => {
                pub fn is_unicast(addr: &Ipv4Addr) -> bool {
                    !addr.is_multicast()
                        && !addr.is_loopback()
                        && !addr.is_link_local()
                        && !addr.is_broadcast()
                        && !addr.is_documentation()
                        && !addr.is_unspecified()
                }

                if !is_unicast(&ip) {
                    return Ok(None);
                }

                let id = self.id;
                let bytes = id.to_be_bytes();
                self.v4.ping(&mut self.b1, IpAddr::V4(ip), &bytes).await?;
                self.id = self.id.wrapping_add(1);
                Ok(Some(id))
            }
            IpAddr::V6(ip) => {
                pub fn is_unicast(addr: &Ipv6Addr) -> bool {
                    !addr.is_multicast()
                        && !addr.is_loopback()
                        && !addr.is_unicast_link_local()
                        && !addr.is_unspecified()
                }

                if !is_unicast(&ip) {
                    return Ok(None);
                }

                let id = self.id;
                let bytes = id.to_be_bytes();
                self.v6.ping(&mut self.b2, IpAddr::V6(ip), &bytes).await?;
                self.id = self.id.wrapping_add(1);
                Ok(Some(id))
            }
        }
    }

    async fn wait_for_result(&mut self) -> Result<(Response, PingKind, u64), Error> {
        let (response, kind, b) = tokio::select! {
            r = self.v4.recv(&mut self.b1) => {
                (r?, PingKind::V4, &self.b1)
            }
            r = self.v6.recv(&mut self.b2) => {
                (r?, PingKind::V6, &self.b2)
            }
        };

        let bytes = *b.read::<[u8; 8]>().context("reading response payload")?;
        let id = u64::from_be_bytes(bytes);
        Ok((response, kind, id))
    }
}

pub(super) async fn new(state: State, hosts: hosts::State) -> Result<(), Error> {
    let mut service = PingerService {
        v4: Pinger::v4()?,
        v6: Pinger::v6()?,
        b1: Buffer::new(),
        b2: Buffer::new(),
        id: 0u64,
    };

    // A host cache.
    let mut cache = HostNameCache::new();
    // Update host list every 10 seconds.
    let mut host_update = time::interval(Duration::from_secs(10));
    // Working set of host ids.
    let mut new = HashSet::new();
    // Hosts we've already seen.
    let mut old = HashSet::new();
    // Domain lookup tasks.
    let mut domain = JoinSet::new();
    // Map of host ids to their domain pinger tasks.
    let mut domains = BTreeMap::<Uuid, Arc<CacheNameResult>>::new();
    // Pending pings.
    let mut deferred = HashMap::<u64, Defer>::new();

    let mut tasks = Tasks::default();
    // Wakeup for next task.
    let mut sleep = pin!(time::sleep_until(Instant::now()));

    loop {
        if let Some(deadline) = tasks.next_deadline() {
            sleep.as_mut().reset(deadline);
        }

        tokio::select! {
            _ = host_update.tick() => {
                cache.evict_old().await;

                new.clear();

                for host in hosts.hosts().await.iter() {
                    new.insert(host.id);

                    let lookup = cache.get(host).await;
                    let id = host.id;

                    domain.spawn(async move {
                        let result = lookup.get().await;
                        (id, result)
                    });
                }

                if new != old {
                    for id in old.difference(&new) {
                        tasks.remove_by_id(*id);
                        domains.remove(id);
                        deferred.retain(|_, d| d.id != *id);
                        state.pinged.lock().await.remove(id);
                    }

                    old.clear();
                    old.extend(new.iter().copied());
                }
            }
            result = domain.join_next(), if !domain.is_empty() => {
                let Some(result) = result else {
                    continue;
                };

                let (id, result) = result.context("domain task panicked")?;
                let new = result.context("domain lookup failed")?;

                if let Some(old) = domains.get(&id) && *new == **old {
                    continue;
                }

                tracing::info!(?id, ?new, "Domain updates");

                tasks.remove_by_id(id);
                deferred.retain(|_, d| d.id != id);

                let mut pinged = state.pinged.lock().await;
                let p = pinged.entry(id).or_default();

                p.errors.clear();
                p.results.clear();

                let now = Instant::now();

                for error in new.errors.iter() {
                    p.error(PingError {
                        error: error.error.to_string(),
                        kind: PingErrorKind::Host(error.name.clone()),
                        sampled: now,
                    });
                }

                for &addr in new.addresses.iter() {
                    tracing::trace!(?id, ?addr, "scheduling ping");
                    tasks.insert(Key { id, addr, deadline: now }, What::Ping);
                }

                domains.insert(id, new.clone());
            }
            result = service.wait_for_result() => {
                let Ok((r, kind, id)) = result else {
                    continue;
                };

                let Some(k) = deferred.remove(&id) else {
                    tracing::trace!(?id, "missing deferred ping response");
                    continue;
                };

                tracing::trace!(?id, ?k.id, ?k.addr, "received ping response");

                tasks.with_mut(k.id, k.addr, async |t| {
                    let now = Instant::now();

                    let mut pinged = state.pinged.lock().await;
                    let p = pinged.entry(k.id).or_default();

                    p.result(PingResult {
                        kind,
                        outcome: r.outcome,
                        code: r.code,
                        sequence: r.sequence,
                        rtt: now.saturating_duration_since(k.started),
                        sampled: now,
                        target: k.addr,
                        source: r.source,
                        dest: r.dest,
                        checksum: r.checksum,
                        expected_checksum: r.expected_checksum,
                    });

                    t.key.deadline = (k.started + NEXT).max(now);
                    t.what = What::Ping;
                }).await;
            }
            _ = sleep.as_mut(), if !tasks.is_empty() => {
                let now = Instant::now();

                let remove = tasks.next_task(async |t| {
                    match t.what {
                        What::Ping => {
                            tracing::trace!(?t, "pinging");

                            let ping_id = match service.ping(t.key.addr).await {
                                Ok(ping_id) => ping_id,
                                Err(error) => {
                                    state.pinged.lock().await.entry(t.key.id).or_default().error(PingError {
                                        error: error.to_string(),
                                        kind: PingErrorKind::Address(t.key.addr),
                                        sampled: now,
                                    });

                                    t.key.deadline = now + NEXT;
                                    t.what = What::Ping;
                                    return None;
                                }
                            };

                            let Some(ping_id) = ping_id else {
                                return Some(t.key);
                            };

                            deferred.insert(ping_id, Defer { id: t.key.id, addr: t.key.addr, started: now });

                            t.key.deadline = now + TIMEOUT;
                            t.what = What::Timeout;
                            None
                        }
                        What::Timeout => {
                            let mut p = state.pinged.lock().await;
                            let p = p.entry(t.key.id).or_default();

                            p.error(PingError {
                                error: String::from("timeout"),
                                kind: PingErrorKind::Address(t.key.addr),
                                sampled: now,
                            });

                            t.key.deadline = now + NEXT;
                            t.what = What::Ping;
                            None
                        }
                    }
                }).await;

                if let Some(key) = remove {
                    tasks.remove(key);
                }
            }
        }
    }
}

#[derive(Debug)]
enum What {
    Ping,
    Timeout,
}

#[derive(Debug)]
struct Task {
    key: Key,
    what: What,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Key {
    deadline: Instant,
    id: Uuid,
    addr: IpAddr,
}

#[derive(Debug, Clone, Copy)]
struct Defer {
    id: Uuid,
    addr: IpAddr,
    started: Instant,
}

#[derive(Default)]
struct Tasks {
    modified: bool,
    tasks: HashMap<(Uuid, IpAddr), Task>,
    timeouts: BTreeSet<Key>,
}

impl Tasks {
    fn is_empty(&self) -> bool {
        self.timeouts.is_empty()
    }

    /// Get the next deadline.
    fn next_deadline(&mut self) -> Option<Instant> {
        if !self.modified {
            return None;
        }

        self.modified = false;
        Some(self.timeouts.first()?.deadline)
    }

    fn remove_by_id(&mut self, id: Uuid) {
        self.modified = true;

        self.tasks.retain(|_, t| {
            if t.key.id != id {
                return true;
            };

            self.timeouts.remove(&t.key);
            false
        });
    }

    fn insert(&mut self, key: Key, what: What) {
        self.modified = true;
        self.tasks.insert((key.id, key.addr), Task { key, what });
        self.timeouts.insert(key);
    }

    fn remove(&mut self, key: Key) -> Option<Task> {
        let t = self.tasks.remove(&(key.id, key.addr))?;
        self.modified = true;
        self.timeouts.remove(&t.key);
        Some(t)
    }

    /// Get and modify the next task.
    async fn next_task<O>(&mut self, f: impl AsyncFnOnce(&mut Task) -> O) -> O
    where
        O: Default,
    {
        let Some(key) = self.timeouts.pop_first() else {
            return O::default();
        };

        let Some(t) = self.tasks.get_mut(&(key.id, key.addr)) else {
            return O::default();
        };

        self.modified = true;
        let out = f(t).await;
        self.timeouts.insert(t.key);
        out
    }

    async fn with_mut(&mut self, id: Uuid, addr: IpAddr, f: impl AsyncFnOnce(&mut Task)) {
        let Some(t) = self.tasks.get_mut(&(id, addr)) else {
            return;
        };

        self.modified = true;
        self.timeouts.remove(&t.key);
        f(t).await;
        self.timeouts.insert(t.key);
    }
}
