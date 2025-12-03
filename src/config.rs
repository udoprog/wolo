use core::cell::RefCell;
use core::fmt::Write;
use core::str::FromStr;
use core::{fmt, iter};

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use macaddr::MacAddr6;
use toml::Value;

trait TakeFlexible
where
    Self: Sized,
{
    fn take_table(key: &str, value: Parser<'_>) -> Option<Self>;

    fn take_value(hosts: Parser<'_>) -> Option<Self>;
}

/// Loaded configuration file.
#[derive(Default)]
pub struct Config {
    /// Address and port to bind the server to.
    pub bind: Option<String>,
    /// Paths to load landing page configuration from.
    pub home: Vec<PathBuf>,
    /// Loaded hosts.
    pub hosts: Vec<HostConfig>,
    /// Paths to load Mokuro files from.
    pub mokuro: Vec<MokuroConfig>,
}

impl Config {
    /// Push mokuro path.
    pub fn push_mokuro_path(&mut self, path: &Path) {
        self.mokuro.push(MokuroConfig {
            path: path.to_owned(),
        });
    }
}

/// Loaded host configuration.
#[derive(Debug)]
pub struct HostConfig {
    /// Loaded host configurations.
    pub macs: BTreeSet<MacAddr6>,
    /// Host names.
    pub names: BTreeSet<String>,
    /// Preferred host name.
    pub preferred_name: Option<String>,
    /// Whether to ignore this host.
    pub ignore: bool,
}

impl TakeFlexible for HostConfig {
    fn take_table(key: &str, mut parser: Parser<'_>) -> Option<Self> {
        let out = Self {
            macs: parser.take_iter("macs"),
            names: BTreeSet::from([key.to_owned()]),
            preferred_name: parser.take("preferred_name"),
            ignore: parser.take_boolean("ignore").unwrap_or(false),
        };

        parser.check();
        Some(out)
    }

    fn take_value(parser: Parser<'_>) -> Option<Self> {
        let names = BTreeSet::from([parser.parse()?]);

        Some(Self {
            macs: BTreeSet::new(),
            names,
            preferred_name: None,
            ignore: false,
        })
    }
}

/// Loaded mokuro configuration.
#[derive(Debug)]
pub struct MokuroConfig {
    /// Mokuro path.
    pub path: PathBuf,
}

impl TakeFlexible for MokuroConfig {
    fn take_table(key: &str, parser: Parser<'_>) -> Option<Self> {
        let out = Self {
            path: PathBuf::from(key),
        };

        parser.check();
        Some(out)
    }

    fn take_value(parser: Parser<'_>) -> Option<Self> {
        Some(Self {
            path: parser.parse()?,
        })
    }
}

impl Config {
    pub fn add_host(&mut self, new: HostConfig) {
        let host = 'found: {
            for host in &mut self.hosts {
                if new.names.iter().any(|n| host.names.contains(n)) {
                    break 'found host;
                }

                if new.macs.iter().any(|m| host.macs.contains(m)) {
                    break 'found host;
                }
            }

            self.hosts.push(new);
            return;
        };

        for mac in new.macs {
            host.macs.insert(mac);
        }

        for name in new.names {
            host.names.insert(name);
        }

        host.preferred_name = new.preferred_name.or(host.preferred_name.take());
        host.ignore |= new.ignore;
    }

    /// Add to configuration from the given path.
    pub fn add_from_path(&mut self, path: &Path, diag: &Diagnostics) -> Result<()> {
        let Ok(bytes) = fs::read(path) else {
            return Ok(());
        };

        let value: Value = toml::from_slice(&bytes).context("failed to parse config file")?;
        let mut parser = Parser::new(value, diag);

        if let Some(bind) = parser.take("bind") {
            self.bind = Some(bind);
        }

        self.home = parser.take_iter("home");

        for host in parser.take_flexible::<HostConfig, Vec<_>>("hosts") {
            self.add_host(host);
        }

        for mokuro in parser.take_flexible::<MokuroConfig, Vec<_>>("mokuro") {
            self.mokuro.push(mokuro);
        }

        parser.check();
        Ok(())
    }

    /// Specify that a given host should be ignored.
    pub fn ignore_host(&mut self, name: &str) {
        let host = 'found: {
            for host in &mut self.hosts {
                if host.names.contains(name) {
                    break 'found host;
                }
            }

            self.hosts.push(HostConfig {
                macs: BTreeSet::new(),
                names: BTreeSet::from([name.to_owned()]),
                preferred_name: None,
                ignore: true,
            });

            return;
        };

        host.ignore = true;
    }
}

#[must_use = "Parser must be consumed to maintain diagnostics"]
struct Parser<'a> {
    value: Value,
    diag: &'a Diagnostics,
}

impl<'a> Parser<'a> {
    fn new(value: Value, diag: &'a Diagnostics) -> Self {
        Self { value, diag }
    }

    fn parse<T>(self) -> Option<T>
    where
        T: FromStr<Err: fmt::Display>,
    {
        let out = match self.value {
            Value::String(value) => match value.parse::<T>() {
                Ok(value) => Some(value),
                Err(error) => {
                    self.diag.error(format_args!("{error}"));
                    None
                }
            },
            other => {
                self.diag
                    .error(format_args!("expected string, found {}", other.type_str()));
                None
            }
        };

        self.diag.pop();
        out
    }

    fn take_any<T>(&mut self, key: &str, parser: impl FnOnce(Value) -> T) -> T
    where
        T: Default,
    {
        let Value::Table(table) = &mut self.value else {
            return T::default();
        };

        let Some(value) = table.remove(key) else {
            return T::default();
        };

        self.diag.key(key);
        let value = parser(value);
        self.diag.pop();
        value
    }

    fn take_iter<T, U>(&mut self, key: &str) -> U
    where
        T: FromStr<Err: fmt::Display>,
        U: FromIterator<T> + Default,
    {
        self.take_any(key, |value| match value {
            Value::String(value) => match value.parse::<T>() {
                Ok(value) => U::from_iter([value]),
                Err(error) => {
                    self.diag.error(format_args!("{error}"));
                    U::default()
                }
            },
            Value::Array(values) => {
                let mut iter = values.into_iter().enumerate();

                let it = iter::from_fn(|| {
                    let (index, value) = iter.next()?;
                    self.diag.index(index);

                    let value = match value {
                        Value::String(value) => match value.parse::<T>() {
                            Ok(value) => Some(value),
                            Err(error) => {
                                self.diag.error(format_args!("{error}"));
                                None
                            }
                        },
                        other => {
                            self.diag
                                .error(format_args!("expected string, found {}", other.type_str()));
                            None
                        }
                    };

                    self.diag.pop();
                    value
                });

                U::from_iter(it)
            }
            other => {
                self.diag
                    .error(format_args!("expected string, found {}", other.type_str()));
                U::default()
            }
        })
    }

    fn take<T>(&mut self, key: &str) -> Option<T>
    where
        T: FromStr<Err: fmt::Display>,
    {
        self.take_any(key, |value| match value {
            Value::String(value) => match value.parse::<T>() {
                Ok(value) => Some(value),
                Err(error) => {
                    self.diag.error(format_args!("{error}"));
                    None
                }
            },
            other => {
                self.diag
                    .error(format_args!("expected string, found {}", other.type_str()));
                None
            }
        })
    }

    fn take_boolean(&mut self, key: &str) -> Option<bool> {
        self.take_any(key, |value| match value {
            Value::Boolean(value) => Some(value),
            other => {
                self.diag
                    .error(format_args!("expected boolean, found {}", other.type_str()));
                None
            }
        })
    }

    fn take_flexible<T, U>(&mut self, key: &str) -> U
    where
        T: TakeFlexible,
        U: FromIterator<T> + Default,
    {
        self.take_any(key, |value| match value {
            Value::Table(table) => {
                let mut it = table.into_iter();

                let it = iter::from_fn(|| {
                    loop {
                        let (key, value) = it.next()?;
                        self.diag.key(&key);

                        let Some(value) = T::take_table(&key, Parser::new(value, self.diag)) else {
                            continue;
                        };

                        return Some(value);
                    }
                });

                U::from_iter(it)
            }
            Value::Array(values) => {
                let mut it = values.into_iter().enumerate();

                let it = iter::from_fn(|| {
                    loop {
                        let (index, value) = it.next()?;
                        self.diag.index(index);

                        let Some(value) = T::take_value(Parser::new(value, self.diag)) else {
                            continue;
                        };

                        return Some(value);
                    }
                });

                U::from_iter(it)
            }
            value => {
                self.diag.error(format_args!(
                    "expected table or array, found {}",
                    value.type_str()
                ));

                U::default()
            }
        })
    }

    fn check(self) {
        match self.value {
            Value::Table(table) => {
                for (key, value) in table {
                    self.diag.key(&key);
                    self.diag
                        .error(format_args!("unexpected key of type {}", value.type_str()));
                    self.diag.pop();
                }
            }
            value => {
                self.diag.error(format_args!(
                    "unexpected value of type {}",
                    value.type_str()
                ));
            }
        }

        self.diag.pop();
    }
}

enum Step {
    Key(String),
    Index(usize),
}

struct DiagnosticsInner {
    path: Vec<Step>,
    errors: Vec<String>,
}

/// Collected diagnostics.
pub struct Diagnostics {
    inner: RefCell<DiagnosticsInner>,
}

impl Diagnostics {
    /// Construct new empty diagnostics.
    pub fn new() -> Self {
        Self {
            inner: RefCell::new(DiagnosticsInner {
                path: Vec::new(),
                errors: Vec::new(),
            }),
        }
    }

    /// Convert into errors.
    pub(crate) fn into_errors(self) -> Vec<String> {
        self.inner.into_inner().errors
    }
}

impl Diagnostics {
    fn index(&self, index: usize) {
        self.inner.borrow_mut().path.push(Step::Index(index));
    }

    fn key(&self, key: &str) {
        self.inner.borrow_mut().path.push(Step::Key(key.to_owned()));
    }

    fn pop(&self) {
        self.inner.borrow_mut().path.pop();
    }

    fn error(&self, message: impl fmt::Display) {
        let mut error = String::new();
        let mut this = self.inner.borrow_mut();

        for step in &this.path {
            match step {
                Step::Key(key) => {
                    error.push('.');

                    if key.contains('.') {
                        error.push('"');
                        error.push_str(key);
                        error.push('"');
                    } else {
                        error.push_str(key);
                    }
                }
                Step::Index(index) => {
                    error.push('[');
                    error.push_str(&index.to_string());
                    error.push(']');
                }
            }
        }

        if !error.is_empty() {
            error.push_str(": ");
        }

        _ = write!(error, "{}", message);
        this.errors.push(error);
    }
}
