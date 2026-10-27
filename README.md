# Oriel display

Oriel display draws web pages in Kodi's GUI while browsing and over video.
It imports buffers rendered by WebKit for the
[Oriel framework](https://github.com/GR3ee3N/oriel-framework). Apps use the
framework API to control the page and playback.

## Status

> [!WARNING]
> **Experimental software.** CoreELEC or Kodi updates can break it.

No support is provided.

## Hardware

Requires an Amlogic device with a Mali GPU running CoreELEC 22
(Kodi 22, 64-bit). Tested on:

- Ugoos AM9 Pro (S905X5M, Mali-G310)
- Ugoos AM6B+ (S922X, Mali-G52)

Other devices and CoreELEC versions have not been tested.

## Installation

For manual installation, download `vfs.oriel.display-<version>.zip` from
the [releases page](https://github.com/GR3ee3N/oriel-display/releases), then:

1. In Kodi, enable **Settings → System → Add-ons → Unknown sources**.
2. Go to **Add-ons → Install from zip file** and select the downloaded ZIP.

Apps can install this add-on as a dependency through the
[Oriel repository](https://github.com/GR3ee3N/oriel-repository).

## Rendering and control

Each app uses two Kodi windows with a `renderaddon` control:

- **browse**: a standalone window that shows the app's startup mark until
  the page reports its first frame.
- **player**: a dialog over Kodi's fullscreen video.

Both windows draw the same imported buffers and redraw when the page
changes.

Two Unix sockets connect the add-on to the framework. The shell sends
WebKit dma-bufs over the producer socket. Python opens and closes windows
and receives keys over the control socket. [docs/protocol.md](docs/protocol.md)
defines both protocols.

## Checks and builds

Use Linux on x86_64 or ARM64, or a Mac with Apple silicon, with Docker
running and `flock` installed (`brew install flock` on a Mac). From the
repository root:

```sh
tools/check.sh
tools/build.sh
```

`tools/check.sh` runs ShellCheck, Python tests, rustfmt, Clippy and Rust
unit tests. `tools/build.sh` compiles the ARM64 libraries and validates
them against `build/box.env`. It stages the add-on in
`.cache/runtime/vfs.oriel.display` and the protocol crate in
`.cache/runtime/protocol`. Set `DISPLAY_DEV` to the staged protocol directory
when building the framework against local display changes.

## Device testing

After building, deploy to a CoreELEC device over SSH:

```sh
BOX=root@coreelec.local tools/deploy.sh
```

Set `BOX` to the device's SSH address. Deployment requires SSH, rsync and
Kodi's JSON-RPC TCP service. The script waits for add-on registration and
verifies activation on each installation attempt.

Deployment reloads a running add-on, closing its windows. On the box,
`tools/control.py` sends control protocol messages for testing.
`crates/addon/src/lib.rs` documents the `orieldisplay://` debug switches.

## Releases

Bump `VERSION` in `build/versions.env` and commit. From a clean working tree,
with the GitHub CLI (`gh`) authenticated to publish releases, run:

```sh
tools/release.sh "Release notes"
```

The script uploads the add-on ZIP, `addons.xml`, protocol crate, vendored
source archive and checksums as a draft release under `v<version>`.
