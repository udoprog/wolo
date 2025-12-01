use core::time::Duration;
use std::collections::{BTreeSet, HashMap, btree_set};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use macaddr::MacAddr6;
use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::{RwLock, RwLockReadGuard};
use tokio::time;
use twox_hash::xxhash3_128;
use uuid::Uuid;

use crate::config::Config;

/// Builder for the host monitoring state.
pub struct Builder {
    ether_paths: Vec<PathBuf>,
    host_paths: Vec<PathBuf>,
}

impl Builder {
    /// Add an /etc/ethers file to monitor.
    pub fn add_ethers_path(&mut self, path: &Path) {
        self.ether_paths.push(path.to_owned());
    }

    /// Add an /etc/hosts file to monitor.
    pub fn add_hosts_path(&mut self, path: &Path) {
        self.host_paths.push(path.to_owned());
    }

    /// Build the host monitoring state.
    pub fn build(self) -> State {
        let inner = Inner {
            ether_paths: self.ether_paths,
            host_paths: self.host_paths,
            hosts: RwLock::new(Vec::new()),
        };

        State {
            inner: Arc::new(inner),
        }
    }
}

struct Inner {
    ether_paths: Vec<PathBuf>,
    host_paths: Vec<PathBuf>,
    hosts: RwLock<Vec<Host>>,
}

/// Representation of a host on the network.
#[derive(Debug, PartialEq)]
pub struct Host {
    pub id: Uuid,
    pub names: BTreeSet<String>,
    pub macs: BTreeSet<MacAddr6>,
    pub preferred_name: Option<String>,
    pub ignore: bool,
}

impl Host {
    /// Get an iterator over the host names.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        let (head, tail) = if let Some(preferred) = &self.preferred_name {
            (Some(preferred.as_str()), btree_set::Iter::default())
        } else {
            (None, self.names.iter())
        };

        head.into_iter().chain(tail.map(|n| n.as_str()))
    }

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

        let bytes = (self.macs.len() as u64).to_be_bytes();
        hasher.write(&bytes);

        for mac in &self.macs {
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
            host_paths: Vec::new(),
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

    /// Read a hosts file from the given path.
    async fn read_hosts(&mut self, path: &Path) -> Vec<String> {
        let Ok(f) = File::open(path).await else {
            return Vec::new();
        };

        let mut reader = BufReader::new(f);
        let mut hosts = Vec::new();

        loop {
            self.line.clear();

            let Ok(n) = reader.read_line(&mut self.line).await else {
                break;
            };

            if n == 0 {
                break;
            }

            let line = self.line.trim();

            if line.starts_with('#') {
                continue;
            }

            let Some((ip, names)) = line.split_once(' ') else {
                continue;
            };

            let Ok(ip) = ip.parse::<IpAddr>() else {
                continue;
            };

            if ip.is_loopback() || ip.is_multicast() || ip.is_unspecified() {
                continue;
            }

            for name in names.split_ascii_whitespace() {
                hosts.push(name.to_owned());
            }
        }

        hosts
    }
}

struct Service {
    by_mac: HashMap<MacAddr6, usize>,
    by_name: HashMap<String, usize>,
    reader: Reader,
}

impl Service {
    fn add_from_config(&mut self, hosts: &mut Vec<Host>, config: &Config) {
        for h in &config.hosts {
            self.add(
                hosts,
                h.macs.iter().copied(),
                &h.names,
                h.preferred_name.as_deref(),
                h.ignore,
            );
        }
    }

    fn add(
        &mut self,
        hosts: &mut Vec<Host>,
        macs: impl IntoIterator<Item = MacAddr6> + Clone,
        names: impl IntoIterator<Item: AsRef<str>> + Clone,
        preferred_name: Option<&str>,
        ignore: bool,
    ) {
        let mut indexes = BTreeSet::new();

        // Try to find existing indexes first.
        for mac in macs.clone() {
            indexes.extend(self.by_mac.get(&mac).copied());
        }

        for name in names.clone() {
            indexes.extend(self.by_name.get(name.as_ref()).copied());
        }

        if indexes.is_empty() {
            let index = hosts.len();

            hosts.push(Host {
                names: names
                    .clone()
                    .into_iter()
                    .map(|n| n.as_ref().to_owned())
                    .collect(),
                macs: macs.clone().into_iter().collect(),
                preferred_name: preferred_name.map(|n| n.to_owned()),
                id: Uuid::nil(),
                ignore,
            });

            indexes.insert(index);
        } else {
            for &index in &indexes {
                let host = &mut hosts[index];
                host.macs.extend(macs.clone().into_iter());
                host.names
                    .extend(names.clone().into_iter().map(|n| n.as_ref().to_owned()));
                host.preferred_name = preferred_name
                    .map(|n| n.to_owned())
                    .or(host.preferred_name.take());
                host.ignore = ignore || host.ignore;
            }
        }

        for mac in macs {
            for &index in &indexes {
                self.by_mac.insert(mac, index);
            }
        }

        for name in names {
            for &index in &indexes {
                self.by_name.insert(name.as_ref().to_owned(), index);
            }
        }
    }
}

/// Spawn the host monitoring task.
pub async fn spawn(state: State, config: Arc<Config>) {
    let mut hosts = Vec::new();

    let mut service = Service {
        by_mac: HashMap::new(),
        by_name: HashMap::new(),
        reader: Reader::default(),
    };

    loop {
        hosts.clear();

        service.by_mac.clear();
        service.by_name.clear();

        for path in &state.inner.ether_paths {
            let ethers = service.reader.read_ethers(path).await;

            for (mac, name) in ethers {
                service.add(&mut hosts, [mac], [name.as_str()], None, false);
            }
        }

        for path in &state.inner.host_paths {
            let found = service.reader.read_hosts(path).await;

            for name in found {
                service.add(&mut hosts, [], [name.as_str()], None, false);
            }
        }

        service.add_from_config(&mut hosts, &config);

        hosts.retain(|h| !h.ignore);

        for host in &mut hosts {
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

            tracing::info!("Updated hosts");

            drop(existing);
            let mut write = state.inner.hosts.write().await;
            *write = hosts.drain(..).collect();
        };

        time::sleep(Duration::from_secs(30)).await;
    }
}
