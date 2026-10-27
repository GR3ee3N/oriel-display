# Protocol

The add-on serves two Unix `SOCK_SEQPACKET` sockets in Linux's abstract
namespace:

- `oriel-display`: the framework shell sends WebKit buffers.
- `oriel-display-control`: framework Python controls windows and receives
  remote actions.

`oriel-display-protocol` in `crates/protocol` defines the Rust wire format.
Releases include a standalone crate archive for framework shell builds.

Each socket serves one client at a time; later clients wait for it to
disconnect. The first packet must contain a hello with the client's protocol
version. The add-on accepts a matching version with its own hello, or
refuses the handshake and closes the connection.

## Producer

Producer protocol version: 1

Each message is a 64-byte packet with little-endian fields:

| Offset | Field | Type | Meaning |
|---|---|---|---|
| 0 | `kind` | u32 | Message type listed below. |
| 4 | `id` | u32 | Nonzero producer-assigned buffer id; protocol version for `hello` and `refused`. |
| 8 | `width` | i32 | `buffer` only |
| 12 | `height` | i32 | `buffer` only |
| 16 | `format` | u32 | `buffer` only: DRM fourcc format. |
| 20 | `planes` | u32 | `buffer` only: plane count, 1 to 4. |
| 24 | `offsets` | u32[4] | `buffer` only: byte offset for each plane. |
| 40 | `strides` | u32[4] | `buffer` only: byte stride for each plane. |
| 56 | `modifier` | u64 | `buffer` only: DRM format modifier. |

Message types:

| Type | Name | From | Meaning |
|---|---|---|---|
| 1 | `buffer` | producer | Import a buffer. Attach one fd per plane using `SCM_RIGHTS`; the add-on closes the fds after import. |
| 2 | `frame` | producer | Select buffer `id` for display starting with the next GUI frame. |
| 3 | `forget` | producer | Remove buffer `id` and its imported texture. |
| 4 | `hello` | producer | Handshake with the protocol version in `id`. On acceptance, the add-on sends `hello`, then `shown` or `hidden`. |
| 10 | `drawn` | add-on | Confirm the first GUI draw of buffer `id` after its `frame`; see the exceptions under Buffer lifecycle below. |
| 11 | `released` | add-on | Confirm the GPU has finished reading buffer `id`; the producer may render into it again. |
| 12 | `refused` | add-on | Reject the protocol version and close the connection; `id` contains the supported protocol version. |
| 13 | `hidden` | add-on | No window is drawing the page; rendering may pause. |
| 14 | `shown` | add-on | A window is drawing the page. |
| 15 | `reset` | add-on | All windows closed: imports were dropped and held buffers released. Resend `buffer` for each buffer before displaying it again. |

Buffer lifecycle:

- Send `buffer` once, then display it by id as needed until `reset`.
  Imports survive transitions between browse and player windows. Closing
  the last window frees every import and its graphics memory.
- After `frame`, the producer must not modify the buffer until `released`.
- A pending `frame` superseded before drawing receives immediate `drawn`
  and `released` replies without being drawn. The same applies to an
  unknown buffer id, such as a frame sent before the producer receives
  `reset` or a buffer whose GPU import or texture binding failed.
- The previous buffer receives `released` after an EGL fence following its
  last draw signals completion. If fence creation fails, the add-on waits
  for GPU completion before releasing it.
- Use premultiplied pixels and finish rendering before submitting a frame;
  the add-on reads the selected buffer immediately.
- Producer disconnection drops all held buffers.

The shell keeps WebKit one frame ahead of Kodi. On `drawn`, it reports
frame completion to WebKit and sends the next pending `frame`.

## Control

Control protocol version: 1

Each packet contains one UTF-8 JSON object.

To the add-on:

| Message | Meaning |
|---|---|
| `{"hello": 1}` | Begin the handshake with the control protocol version. |
| `{"open": "browse", "mark": PATH}` | Show the standalone browse window with the page over black. Centre the optional image at `PATH` until `started`. Kodi leaves this window when playback starts. |
| `{"open": "player"}` | Show the page in a dialog over Kodi's fullscreen video. |
| `{"close": "browse"}`, `{"close": "player"}` | Close the named window. |
| `{"started": true}` | Hide the startup mark after the page renders its first frame. |
| `{"draw": false}`, `{"draw": true}` | Suspend or resume page drawing, such as while a Kodi dialog is open. |

From the add-on:

| Message | Meaning |
|---|---|
| `{"hello": 1}` | Accept the handshake. |
| `{"error": WHY, "hello": 1}` | Refuse the handshake and close the connection; include the supported control protocol version. |
| `{"error": WHY, "message": MESSAGE}` | Report an unhandled message and its rejection reason. |
| `{"window": "browse", "id": ID, "state": "opened"}` | Report an opened window with its Kodi window id. |
| `{"window": "browse", "state": "closed"}` | Report window closure requested by the client or Kodi. |
| `{"key": ACTION, "t": SECONDS, "kodi": BOOL}` | Report a Kodi action id, `CLOCK_MONOTONIC` timestamp and whether Kodi also handles it. Page keys are consumed except Kodi playback keys in the player dialog. Other actions, including volume and screenshots, reach Kodi. |
| `{"gone": true}` | Report add-on unload after requesting window closure. |

Control disconnection schedules window closure on Kodi's GUI thread,
starting with the next GUI frame, to return input control to Kodi. Browse
closure waits for modal dialogs to close. If Kodi restores an app window
from its history while no control client is connected, that window also
schedules its own closure.

## Unloading

The add-on library remains mapped so Kodi can safely invoke window
callbacks after unload. Shutdown requests window closure on the GUI
thread, waits up to 0.5 s, sends `gone` to the control client and closes
both sockets.
