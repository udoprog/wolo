use core::cell::RefCell;
use core::fmt;
use core::fmt::Write;
use core::str::FromStr;

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use macaddr::MacAddr6;
use toml::Value;

/// Loaded configuration file.
#[derive(Default)]
pub struct Config {
    /// Address and port to bind the server to.
    pub bind: Option<String>,
    /// Loaded hosts.
    pub hosts: Vec<HostConfig>,
}

/// Loaded host configuration.
pub struct HostConfig {
    /// Loaded host configurations.
    pub macs: BTreeSet<MacAddr6>,
    /// Host names.
    pub names: BTreeSet<String>,
    /// Preferred host name.
    pub preferred_name: Option<String>,
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
    }

    /// Add to configuration from the given path.
    pub fn add_from_path(&mut self, path: &Path, diag: &Diagnostics) -> Result<()> {
        let Ok(bytes) = fs::read(path) else {
            return Ok(());
        };

        let value: Value = toml::from_slice(&bytes).context("failed to parse config file")?;
        let mut parser = Parser::new(value, diag);

        if let Some(bind) = parser.take("bind", Parser::parse).flatten() {
            self.bind = Some(bind);
        }

        parser.take("hosts", |hosts| match hosts.value {
            Value::Table(table) => {
                for (key, value) in table {
                    hosts.diag.key(&key);
                    let mut parser = Parser::new(value, hosts.diag);

                    self.add_host(HostConfig {
                        macs: parser
                            .take("macs", |p| p.iter(Parser::parse))
                            .unwrap_or_default(),
                        names: BTreeSet::from([key.to_owned()]),
                        preferred_name: parser.take("preferred_name", Parser::parse).flatten(),
                    });

                    parser.check();
                }
            }
            Value::Array(values) => {
                for (index, value) in values.into_iter().enumerate() {
                    hosts.diag.index(index);

                    if let Some(host) = Parser::new(value, hosts.diag).parse() {
                        self.add_host(HostConfig {
                            macs: BTreeSet::new(),
                            names: BTreeSet::from([host]),
                            preferred_name: None,
                        });
                    }
                }
            }
            Value::String(name) => {
                self.add_host(HostConfig {
                    macs: BTreeSet::new(),
                    names: BTreeSet::from([name.to_owned()]),
                    preferred_name: None,
                });
            }
            other => {
                hosts.diag.error(format_args!(
                    "expected table or string, found {}",
                    other.type_str()
                ));
            }
        });

        parser.check();
        Ok(())
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

    fn take<O>(&mut self, key: &str, parser: impl FnOnce(Parser<'a>) -> O) -> Option<O> {
        let value = match &mut self.value {
            Value::Table(table) => table.remove(key)?,
            _ => return None,
        };

        self.diag.key(key);
        let output = parser(Parser::new(value, self.diag));
        Some(output)
    }

    fn iter<U, O>(self, mut iter: impl FnMut(Parser<'a>) -> Option<O>) -> U
    where
        U: FromIterator<O>,
    {
        let mut out = Vec::new();

        match self.value {
            Value::Array(array) => {
                for (index, value) in array.into_iter().enumerate() {
                    self.diag.index(index);

                    if let Some(o) = iter(Parser::new(value, self.diag)) {
                        out.push(o);
                    }
                }
            }
            other => {
                self.diag
                    .error(format_args!("expected array, found {}", other.type_str()));
            }
        }

        self.diag.pop();
        U::from_iter(out)
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
