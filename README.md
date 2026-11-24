# jdctl

Control [JDownloader 2](https://jdownloader.org/) from the command line: follow the downloads,
start and pause them, send links to the link grabber and act on packages and links. It talks to
JDownloader either directly on the network (its local API) or through MyJDownloader, so a
JDownloader behind a NAT or in a container works too.

jdctl is also a [Canopy](https://github.com/AlexArtaud-Dev/Canopy) plugin: installed there, it
brings a **Downloads** page (downloads, link grabber, actions on chosen rows) and a header pill
while downloads run.

## Install

Download the binary for your platform from the
[latest release](https://github.com/AlexArtaud-Dev/jdctl/releases/latest) (static Linux builds
for amd64 and arm64, macOS arm64, Windows amd64) and check it against `SHA256SUMS`. Or build it:

```sh
cargo install --git https://github.com/AlexArtaud-Dev/jdctl
```

## Connect

A connection is saved once, by name; the first one (or the one marked `--default`) is used when
`--connection` is not given.

**MyJDownloader** (works wherever JDownloader runs, as long as it is signed in to MyJDownloader):

```sh
jdctl connect cloud home --email you@example.com --default   # asks the password
jdctl devices                                                  # if the account has several
```

The password itself is never stored: only the secrets MyJDownloader derives from it.

**Local API** (no account; the machine running jdctl must reach JDownloader): in JDownloader,
*Settings › Advanced Settings*, enable `RemoteAPI: Deprecated API` and restart it; in Docker,
publish port 3128. Then:

```sh
jdctl connect local box --url http://192.168.1.10:3128
```

Connections are kept in `$XDG_CONFIG_HOME/jdctl` (`%APPDATA%\jdctl` on Windows), or in
`CANOPY_PLUGIN_DATA` under Canopy. Set `JDCTL_KEY` (or `CANOPY_PLUGIN_KEY`) to encrypt them.

## Use

```sh
jdctl state                          # packages, links, progress, speed
jdctl add https://example.com/file   # to the link grabber; --start to download at once
jdctl check --package 12             # are the found links online?
jdctl confirm --package 12           # move them to the downloads
jdctl pause                          # start | stop | pause | resume
jdctl disable --link 34,35           # enable | disable | remove | force | reset
jdctl rename "New name" --package 12
jdctl priority highest --link 34
jdctl folder /output/Films --package 12
```

Ids come from `jdctl state`. Options can go anywhere; `--json` prints machine-readable output,
and `jdctl call /NAMESPACE/METHOD [JSON…]` sends any API call (for diagnostics). `jdctl --help`
lists everything.

## Canopy

An administrator adds `AlexArtaud-Dev/jdctl` to the plugin sources and installs it, then saves
a connection with **Connect to MyJDownloader** (or **Connect to JDownloader (network)**) on the
Commands page, in advanced mode. The **Downloads** page is a right of its own: grant it per role
under *Plugin pages*. Needs Canopy 2026.11.44 or later.

## License

MIT
