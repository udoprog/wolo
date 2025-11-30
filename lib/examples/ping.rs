use core::net::{IpAddr, SocketAddr};
use std::net::ToSocketAddrs;

use anyhow::{Context, Result};
use clap::Parser;
use lib::Pinger;

#[derive(Parser)]
struct Opts {
    /// Use IpV4 to ping.
    #[clap(short = '4', conflicts_with = "v6")]
    v4: bool,
    /// Use IpV6 to ping.
    #[clap(short = '6', conflicts_with = "v4")]
    v6: bool,
    /// Destination to ping.
    dest: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let opts = Opts::try_parse()?;

    let filter: fn(&SocketAddr) -> bool = if opts.v6 {
        |a| a.is_ipv6()
    } else if opts.v4 {
        |a| a.is_ipv4()
    } else {
        |_| true
    };

    let dest = (opts.dest.as_str(), 0)
        .to_socket_addrs()?
        .find(filter)
        .context("resolving destination address")?
        .ip();

    let pinger = match dest {
        IpAddr::V4(..) => Pinger::v4()?,
        IpAddr::V6(..) => Pinger::v6()?,
    };

    let mut buf = lib::Buffer::new();

    loop {
        pinger
            .ping(&mut buf, dest, &[0xde, 0xad, 0xbe, 0xef])
            .await?;

        let res = pinger.recv(&mut buf).await?;

        dbg!(res);
        assert_eq!(buf.as_bytes(), &[0xde, 0xad, 0xbe, 0xef]);
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
    }
}
