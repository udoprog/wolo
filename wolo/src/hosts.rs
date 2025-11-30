use core::time::Duration;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use macaddr::MacAddr6;
use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::{RwLock, RwLockReadGuard};
use tokio::time;
use twox_hash::xxhash3_128;
use uuid::Uuid;

/// Builder for the host monitoring state.
pub struct Builder {
    ether_paths: Vec<PathBuf>,
}

impl Builder {
    /// Add a path to an ethers file to monitor.
    pub fn add_ethers_path(&mut self, path: &Path) {
        self.ether_paths.push(path.to_owned());
    }

    /// Build the host monitoring state.
    pub fn build(self) -> State {
        let inner = Inner {
            ether_paths: self.ether_paths,
            hosts: RwLock::new(Vec::new()),
        };

        State {
            inner: Arc::new(inner),
        }
    }
}

struct Inner {
    ether_paths: Vec<PathBuf>,
    hosts: RwLock<Vec<Host>>,
}

/// Representation of a host on the network.
#[derive(Debug, PartialEq)]
pub struct Host {
    pub id: Uuid,
    pub names: Vec<String>,
    pub mac: Vec<MacAddr6>,
}

impl Host {
    pub fn build_id(&mut self) {
        const NAME: u8 = 0x01;
        const MAC: u8 = 0x02;

        let mut hasher = xxhash3_128::Hasher::default();

        let bytes = (self.names.len() as u64).to_be_bytes();
        hasher.write(&bytes);

        for name in &self.names {
            hasher.write(&[NAME]);
            hasher.write(name.as_bytes());
        }

        let bytes = (self.mac.len() as u64).to_be_bytes();
        hasher.write(&bytes);

        for mac in &self.mac {
            hasher.write(&[MAC]);
            hasher.write(mac.as_bytes());
        }

        self.id = Uuid::from_u128(hasher.finish_128());
    }
}

/// State shared between the host monitoring task and the web server.
#[derive(Clone)]
pub struct State {
    inner: Arc<Inner>,
}

impl State {
    /// Get a read lock on the current list of hosts.
    pub async fn hosts(&self) -> RwLockReadGuard<'_, [Host]> {
        let hosts = self.inner.hosts.read().await;
        RwLockReadGuard::map(hosts, |v| v.as_slice())
    }
}

impl State {
    /// Create a new builder for the host monitoring state.
    pub fn builder() -> Builder {
        Builder {
            ether_paths: Vec::new(),
        }
    }
}

#[derive(Default)]
struct Reader {
    line: String,
}

impl Reader {
    /// Read an ethers file from the given path.
    async fn read_ethers(&mut self, path: &Path) -> Vec<(MacAddr6, String)> {
        let Ok(f) = File::open(path).await else {
            return Vec::new();
        };

        let mut reader = BufReader::new(f);
        let mut ethers = Vec::new();

        loop {
            self.line.clear();

            let Ok(n) = reader.read_line(&mut self.line).await else {
                break;
            };

            if n == 0 {
                break;
            }

            let line = self.line.trim();

            let Some((mac, name)) = line.split_once(' ') else {
                continue;
            };

            let Ok(mac) = mac.parse::<MacAddr6>() else {
                continue;
            };

            let name = name.trim();
            ethers.push((mac, name.to_owned()));
        }

        ethers
    }
}

struct Service {
    state: State,
    by_mac: HashMap<MacAddr6, usize>,
    by_name: HashMap<String, usize>,
    reader: Reader,
}

impl Service {
    async fn build_hosts(&mut self, hosts: &mut Vec<Host>) {
        self.by_mac.clear();
        self.by_name.clear();

        hosts.clear();

        for ether in &self.state.inner.ether_paths {
            let ethers = self.reader.read_ethers(ether).await;

            for (mac, name) in ethers {
                let index = self
                    .by_mac
                    .get(&mac)
                    .copied()
                    .or_else(|| self.by_name.get(&name).copied());

                let index = match index {
                    Some(i) => i,
                    None => {
                        let index = hosts.len();

                        hosts.push(Host {
                            names: Vec::new(),
                            mac: Vec::new(),
                            id: Uuid::nil(),
                        });

                        index
                    }
                };

                self.by_mac.entry(mac).or_insert(index);

                if !self.by_name.contains_key(&name) {
                    self.by_name.insert(name.clone(), index);
                }

                let host = &mut hosts[index];

                if !host.names.contains(&name) {
                    host.names.push(name.clone());
                }

                if !host.mac.contains(&mac) {
                    host.mac.push(mac);
                }
            }
        }
    }
}

/// Spawn the host monitoring task.
pub async fn spawn(state: State) {
    let mut hosts = Vec::new();

    let mut service = Service {
        state: state.clone(),
        by_mac: HashMap::new(),
        by_name: HashMap::new(),
        reader: Reader::default(),
    };

    loop {
        service.build_hosts(&mut hosts).await;

        for host in &mut hosts {
            host.names.sort();
            host.mac.sort();
            host.build_id();
        }

        hosts.sort_by_key(|h| h.id);

        let existing = state.inner.hosts.read().await;

        'done: {
            if existing.len() == hosts.len()
                && existing.iter().zip(&hosts).all(|(a, b)| a.id == b.id)
            {
                hosts.clear();
                break 'done;
            }

            tracing::info!("updated hosts");

            drop(existing);
            let mut write = state.inner.hosts.write().await;
            *write = hosts.drain(..).collect();
        };

        time::sleep(Duration::from_secs(30)).await;
    }
}
