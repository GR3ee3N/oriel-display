# Oriel display

Oriel display is a Kodi add-on that draws web pages inside Kodi's own GUI,
both while browsing and over playing video. It displays the buffers WebKit
renders for the [Oriel framework](https://github.com/GR3ee3N/oriel-framework).
Apps use the framework, not this add-on directly.

## Status

> [!WARNING]
> **Oriel display is highly experimental.** It is a personal project, tested
> on only a few boxes, and it relies on CoreELEC and Kodi internals that are
> outside its control. Any update to those can break it. Use it at your own
> risk.

No support is provided.

## Installation

Apps install it as a dependency of the framework through the
[Oriel repository](https://github.com/GR3ee3N/oriel-repository). To install
it by hand, download `vfs.oriel.display-<version>.zip` from the
[releases page](https://github.com/GR3ee3N/oriel-display/releases), enable
**Settings → System → Add-ons → Unknown sources** in Kodi, and use
**Add-ons → Install from zip file**.

## How it works

The add-on gives an Oriel app two Kodi windows, each with a `renderaddon`
control that draws the page: **browse**, a window of its own with the app's
mark until the page starts, and **player**, a dialog over Kodi's fullscreen
video. Both draw the same imported buffers; the player redraws in every GUI
frame, the browse window when the page changes.

Two unix sockets connect it to the framework
([docs/protocol.md](docs/protocol.md)): the framework's shell sends WebKit's
buffers as dma-bufs over the producer socket, and its Python opens and
closes the windows and hears the keys over the control socket.

## Hardware and Kodi

The add-on targets Amlogic devices with a Mali GPU running CoreELEC 22
(Kodi 22, 64-bit). It has been tested on:

- Ugoos AM6B+ (S922X, Mali-G52), Kodi 22.0-BETA2
- Ugoos AM9 Pro (S905X5M, Mali-G310), Kodi 22.0-RC1

## Building

Building requires Linux on x86_64 or ARM64 and a running Docker
installation. Run from the repository root:

```sh
tools/check.sh
tools/build.sh
```

`check.sh` runs ShellCheck, the Python tests, rustfmt, clippy and the Rust
unit tests. `build.sh` compiles the ARM64 library, checks it against
`build/box.env`, and stages the add-on in `.cache/runtime/vfs.oriel.display`
and the protocol crate in `.cache/runtime/protocol`. Pass that directory as
`DISPLAY_DEV` to a framework build when developing both projects.

Deploy to a CoreELEC device with `BOX=root@coreelec.local tools/deploy.sh`;
a running add-on is reloaded. On the box, `tools/control.py` speaks the
control protocol for testing. See `crates/addon/src/lib.rs` for the
`orieldisplay://` debug switches.

## Releasing

Bump `VERSION` in `build/versions.env`, commit, then build and publish from
a clean working tree with the GitHub CLI (`gh`):

```sh
tools/release.sh "Release notes"
```

This uploads the add-on zip, `addons.xml`, the protocol crate, the vendored
source archive, and checksums as a draft release under `v<version>`.
