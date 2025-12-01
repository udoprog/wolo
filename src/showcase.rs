use core::net::IpAddr;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::net::Ipv4Addr;
use std::sync::Arc;

use macaddr::MacAddr6;
use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};
use tokio::sync::{Mutex, MutexGuard};
use uuid::Uuid;

#[derive(Clone)]
enum Inner {
    Enabled(Arc<Mutex<State>>),
    Disabled,
}

#[derive(Clone)]
pub struct Helper {
    inner: Inner,
}

impl Helper {
    /// Lock the inner state.
    pub async fn lock(&self) -> LockedHelper<'_> {
        let inner = match &self.inner {
            Inner::Enabled(state) => LockKind::Enabled(state.lock().await),
            Inner::Disabled => LockKind::Disabled,
        };

        LockedHelper { inner }
    }
}

enum LockKind<'a> {
    Enabled(MutexGuard<'a, State>),
    Disabled,
}

/// A locked showcase helper.
pub struct LockedHelper<'a> {
    inner: LockKind<'a>,
}

impl LockedHelper<'_> {
    /// Get a host name..
    pub fn host_name(&mut self, host_id: Uuid, name: &str) -> String {
        match &mut self.inner {
            LockKind::Enabled(state) => state.host_name(host_id, name),
            LockKind::Disabled => name.to_owned(),
        }
    }

    /// Get a MAC address.
    pub fn mac(&mut self, mac: MacAddr6) -> MacAddr6 {
        match &mut self.inner {
            LockKind::Enabled(state) => state.mac(mac),
            LockKind::Disabled => mac,
        }
    }

    /// Get an IP address.
    pub fn ip(&mut self, ip: IpAddr) -> IpAddr {
        match &mut self.inner {
            LockKind::Enabled(state) => state.ip(ip),
            LockKind::Disabled => ip,
        }
    }
}

#[derive(Default)]
struct State {
    host_to_index: HashMap<Uuid, usize>,
    host_names: HashMap<(Uuid, String), String>,
    mac: HashMap<MacAddr6, MacAddr6>,
    ips: HashMap<IpAddr, IpAddr>,
}

impl State {
    fn host_name(&mut self, host_id: Uuid, name: &str) -> String {
        let key = (host_id, name.to_owned());

        if let Some(existing) = self.host_names.get(&key) {
            return existing.clone();
        }

        let index = self.host_index(host_id);

        let base = match index {
            0 => "desktop".to_owned(),
            1 => "raspberrypi".to_owned(),
            2 => "router".to_owned(),
            3 => "laptop".to_owned(),
            4 => "jumphost".to_owned(),
            _ => format!("host{index}"),
        };

        let showcase_name = match self
            .host_names
            .iter()
            .filter(|((id, _), _)| *id == host_id)
            .count()
        {
            0 => base.to_string(),
            _ => format!("{base}.lan"),
        };

        self.host_names.insert(key, showcase_name.clone());
        showcase_name
    }

    fn mac(&mut self, mac: MacAddr6) -> MacAddr6 {
        if let Some(existing) = self.mac.get(&mac) {
            return *existing;
        }

        let mut rng = SmallRng::seed_from_u64(self.mac.len() as u64);

        let out = MacAddr6::new(
            rng.random(),
            rng.random(),
            rng.random(),
            rng.random(),
            rng.random(),
            rng.random(),
        );

        self.mac.insert(mac, out);
        out
    }

    fn ip(&mut self, ip: IpAddr) -> IpAddr {
        if let Some(existing) = self.ips.get(&ip) {
            return *existing;
        }

        let mut rng = SmallRng::seed_from_u64(self.ips.len() as u64);

        let out = match ip {
            IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::new(
                rng.random(),
                rng.random(),
                rng.random(),
                rng.random(),
            )),
            IpAddr::V6(_) => IpAddr::V6(std::net::Ipv6Addr::new(
                rng.random(),
                rng.random(),
                rng.random(),
                rng.random(),
                rng.random(),
                rng.random(),
                rng.random(),
                rng.random(),
            )),
        };

        self.ips.insert(ip, out);
        out
    }

    fn host_index(&mut self, host_id: Uuid) -> usize {
        let next = self.host_to_index.len();

        match self.host_to_index.entry(host_id) {
            Entry::Occupied(occupied_entry) => *occupied_entry.get(),
            Entry::Vacant(vacant_entry) => {
                vacant_entry.insert(next);
                next
            }
        }
    }
}

/// Construct a new showcase helper.
pub fn new(showcase: bool) -> Helper {
    Helper {
        inner: if showcase {
            Inner::Enabled(Arc::new(Mutex::new(State::default())))
        } else {
            Inner::Disabled
        },
    }
}
