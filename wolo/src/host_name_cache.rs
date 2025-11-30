use core::time::Duration;
use std::collections::HashMap;
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use tokio::sync::RwLock;
use tokio::task::{self, JoinHandle};
use uuid::Uuid;

use crate::hosts::Host;

/// A cache of looked up host names.
pub struct HostNameCache {
    map: Arc<RwLock<HashMap<Uuid, HostNameEntry>>>,
}

impl HostNameCache {
    /// Construct a new host name cache.
    pub fn new() -> Self {
        Self {
            map: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Get an entry from the cache.
    pub async fn get(&self, host: &Host) -> HostNameCacheLookup {
        {
            let map = self.map.read().await;

            if let Some(entry) = map.get(&host.id) {
                return HostNameCacheLookup {
                    kind: InnerKind::Found {
                        results: entry.results.clone(),
                    },
                };
            }
        }

        let names = host.names.clone();

        let handle = task::spawn_blocking(move || {
            let mut results = Vec::new();

            for name in names {
                match (name.as_str(), 0).to_socket_addrs() {
                    Err(error) => {
                        results.push(CacheNameResult::Error(NameError {
                            name,
                            error: format!("{error}"),
                        }));
                        continue;
                    }
                    Ok(result) => {
                        for addr in result {
                            results.push(CacheNameResult::Address(addr));
                        }
                    }
                }
            }

            results
        });

        HostNameCacheLookup {
            kind: InnerKind::Handle {
                id: host.id,
                map: self.map.clone(),
                handle,
            },
        }
    }

    /// Evict errors older than 30 seconds and name entries older than 2 minutes.
    pub async fn evict_old(&mut self) {
        const DURATION: Duration = Duration::from_secs(120);

        let now = Instant::now();

        let mut map = self.map.write().await;
        map.retain(move |_, entry| now.saturating_duration_since(entry.last) <= DURATION);
    }
}

/// A result from a cache name lookup.
#[derive(Clone)]
pub enum CacheNameResult {
    Error(NameError),
    Address(SocketAddr),
}

/// A name lookup error.
#[derive(Clone)]
pub struct NameError {
    /// The name that was looked up.
    pub name: String,
    /// The error during name resolution.
    pub error: String,
}

/// A cache lookup.
pub struct HostNameCacheLookup {
    kind: InnerKind,
}

impl HostNameCacheLookup {
    /// Get the results of the lookup.
    pub async fn get(self) -> Result<Arc<[CacheNameResult]>> {
        match self.kind {
            InnerKind::Found { results } => Ok(results),
            InnerKind::Handle { id, map, handle } => {
                let results = Arc::<[CacheNameResult]>::from(
                    handle.await.context("name lookup task panicked")?,
                );
                let mut map = map.write().await;

                map.insert(
                    id,
                    HostNameEntry {
                        results: results.clone(),
                        last: Instant::now(),
                    },
                );

                Ok(results)
            }
        }
    }
}

enum InnerKind {
    Found {
        results: Arc<[CacheNameResult]>,
    },
    Handle {
        id: Uuid,
        map: Arc<RwLock<HashMap<Uuid, HostNameEntry>>>,
        handle: JoinHandle<Vec<CacheNameResult>>,
    },
}

struct HostNameEntry {
    results: Arc<[CacheNameResult]>,
    last: Instant,
}
