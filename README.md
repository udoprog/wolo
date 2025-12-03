# wolo

[<img alt="github" src="https://img.shields.io/badge/github-udoprog/wolo-8da0cb?style=for-the-badge&logo=github" height="20">](https://github.com/udoprog/wolo)
[<img alt="docs.rs" src="https://img.shields.io/badge/docs.rs-wolo-66c2a5?style=for-the-badge&logoColor=white&logo=data:image/svg+xml;base64,PHN2ZyByb2xlPSJpbWciIHhtbG5zPSJodHRwOi8vd3d3LnczLm9yZy8yMDAwL3N2ZyIgdmlld0JveD0iMCAwIDUxMiA1MTIiPjxwYXRoIGZpbGw9IiNmNWY1ZjUiIGQ9Ik00ODguNiAyNTAuMkwzOTIgMjE0VjEwNS41YzAtMTUtOS4zLTI4LjQtMjMuNC0zMy43bC0xMDAtMzcuNWMtOC4xLTMuMS0xNy4xLTMuMS0yNS4zIDBsLTEwMCAzNy41Yy0xNC4xIDUuMy0yMy40IDE4LjctMjMuNCAzMy43VjIxNGwtOTYuNiAzNi4yQzkuMyAyNTUuNSAwIDI2OC45IDAgMjgzLjlWMzk0YzAgMTMuNiA3LjcgMjYuMSAxOS45IDMyLjJsMTAwIDUwYzEwLjEgNS4xIDIyLjEgNS4xIDMyLjIgMGwxMDMuOS01MiAxMDMuOSA1MmMxMC4xIDUuMSAyMi4xIDUuMSAzMi4yIDBsMTAwLTUwYzEyLjItNi4xIDE5LjktMTguNiAxOS45LTMyLjJWMjgzLjljMC0xNS05LjMtMjguNC0yMy40LTMzLjd6TTM1OCAyMTQuOGwtODUgMzEuOXYtNjguMmw4NS0zN3Y3My4zek0xNTQgMTA0LjFsMTAyLTM4LjIgMTAyIDM4LjJ2LjZsLTEwMiA0MS40LTEwMi00MS40di0uNnptODQgMjkxLjFsLTg1IDQyLjV2LTc5LjFsODUtMzguOHY3NS40em0wLTExMmwtMTAyIDQxLjQtMTAyLTQxLjR2LS42bDEwMi0zOC4yIDEwMiAzOC4ydi42em0yNDAgMTEybC04NSA0Mi41di03OS4xbDg1LTM4Ljh2NzUuNHptMC0xMTJsLTEwMiA0MS40LTEwMi00MS40di0uNmwxMDItMzguMiAxMDIgMzguMnYuNnoiPjwvcGF0aD48L3N2Zz4K" height="20">](https://docs.rs/wolo)
[<img alt="build status" src="https://img.shields.io/github/actions/workflow/status/udoprog/wolo/ci.yml?branch=main&style=for-the-badge" height="20">](https://github.com/udoprog/wolo/actions?query=branch%3Amain)
[<img alt="chat on discord" src="https://img.shields.io/discord/558644981137670144.svg?logo=discord&style=flat-square" height="20">](https://discord.gg/v5AeNkT)

Simple network monitor capable of sending magic Wake-on-LAN packets.

Populate `/etc/ethers` (`man ethers`) and/or `/etc/hosts` (`man hosts`) and
run with:

```sh
wolo --bind 127.0.0.1:3000 --home home.md
```

The `home.md` is used to populate the landing page, see [Landing
Page](#landing-page) below for how to configure this.

The `/network` page show an overview of the state of hosts on the network
and the ability to wake them up if they have configured mac addresses.

<table>
<tr>
<td valign="top"><img alt="Default Landing Page" src="https://github.com/udoprog/wolo/blob/main/gfx/home.png?raw=true" /></td>
<td valign="top"><img alt="Network Page" src="https://github.com/udoprog/wolo/blob/main/gfx/network.png?raw=true" /></td>
<td valign="top"><img alt="Network Page in lynx" src="https://github.com/udoprog/wolo/blob/main/gfx/lynx.png?raw=true" /></td>
</td>
</table>

> **wolo** has a reactive design which works well on mobiles and all the
> pages work with a basic browser *without* JavaScript.

<br>

## Configuration

The wolo service can take configuration from multiple sources:

* By default we parse `/etc/hosts` to find hosts to interact with.
  Additional hosts files can be specified using `--hosts <path>`.
* By default we parse `/etc/ethers` to find and associate hosts with MAC
  addresses. Additional files of this format can be specified using
  `--ethers <path>`.
* Any number of optional configuration files can be specified using
  `--config <path>`.

The configuration files are in toml, and have the following format:

```toml
# The default socket address to bind to.
# Can be IPv4 or IPv6.
bind = "localhost:3000"

# Simple variant of a list of hosts.
hosts = ["example.com", "another.example.com"]

# Detailed host configuration.
[hosts."example.com"]
# Collection of mac addresses associated with this host.
macs = ["00:11:22:33:44:55"]
# Setting the preferred name will make it so that only this name is
# displayed in the network view for this host.
preferred_name = "example"
# Whether this host should be ignored.
#
# Additional hosts to be ignored can be specified with the
# `--ignore-host` option.
ignore = false
```

<br>

#### Landing Page

We expect a landing page to be specified in markdown either through the
`home` option or the `--home` cli option. This can be dynamically changed
while the service is running.

```md
# wolo

This is the landing page for your wolo installation. Please edit it by copying
it from the README.md and specify an alternative path using the --home option.

* [Network](/network)
* [Github](https://github.com/udoprog/wolo)
```

Note that arbitrary markdown is not supported. Only the given structures are
supported. The first title, paragraphs and links in list will simply be
extracted and used to build the landing page. Warnings will be emitted for
entries which are currently skipped.
