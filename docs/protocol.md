# Protocol

The add-on serves two unix sockets in the abstract namespace, both
`SOCK_SEQPACKET`:

- `oriel-display`, for the producer: the framework's shell, which sends
  WebKit's buffers.
- `oriel-display-control`, for the control client: the framework's Python,
  which opens and closes the windows and hears keys.

The `oriel-display-protocol` crate (`crates/protocol`) defines the wire
format for Rust. Each release publishes it as a standalone archive, which the
framework's shell builds against.

Each socket takes one client at a time; a second one waits until the first
disconnects. A client's first message is a hello with the protocol version it
speaks. The add-on answers with a hello of the same version, or refuses and
closes the connection.

## Producer

Producer protocol version: 1

Every message is one 64-byte packet, little-endian, in this layout:

| Offset | Field | Type | Meaning |
|---|---|---|---|
| 0 | `kind` | u32 | message type, below |
| 4 | `id` | u32 | buffer id, chosen by the producer, never 0; the version in `hello` and `refused` |
| 8 | `width` | i32 | `buffer` only |
| 12 | `height` | i32 | `buffer` only |
| 16 | `format` | u32 | `buffer` only: DRM fourcc |
| 20 | `planes` | u32 | `buffer` only: 1 to 4 |
| 24 | `offsets` | u32[4] | `buffer` only: per plane |
| 40 | `strides` | u32[4] | `buffer` only: per plane |
| 56 | `modifier` | u64 | `buffer` only: DRM format modifier |

Message types:

| Type | Name | From | Meaning |
|---|---|---|---|
| 1 | `buffer` | producer | A new buffer. One fd per plane rides along as `SCM_RIGHTS`; the add-on closes them after importing. |
| 2 | `frame` | producer | Show buffer `id` from the next GUI frame. |
| 3 | `forget` | producer | Buffer `id` is gone; the add-on drops its texture. |
| 4 | `hello` | producer | The first message: `id` is the version. The add-on answers with its own `hello`, then `shown` or `hidden`. |
| 10 | `drawn` | add-on | Buffer `id` was drawn into a GUI frame for the first time since its `frame`. |
| 11 | `released` | add-on | The GPU no longer reads buffer `id`; the producer may render into it again. |
| 12 | `refused` | add-on | The producer's version isn't spoken; `id` is the add-on's. The connection closes. |
| 13 | `hidden` | add-on | No window draws the page now; the producer may stop rendering. |
| 14 | `shown` | add-on | A window draws the page again. |
| 15 | `reset` | add-on | No window is open, so the add-on dropped every buffer and released the ones it held. The producer sends `buffer` again for each one before showing it. |

Rules:

- A buffer is sent once with `buffer`, then shown by id as often as needed,
  until a `reset`. The buffers stay imported while a window is open, also
  while the page moves from one window to the other; when the last window
  closes, the add-on drops them, so that Kodi holds no graphics memory for a
  page nobody sees.
- After `frame`, the buffer belongs to the add-on until `released`.
- A `frame` that arrives before the previous one was drawn replaces it; the
  add-on then reports the replaced buffer `drawn` and `released` at once. So
  it does for a `frame` naming a buffer it doesn't have, as one sent before
  a `reset` reached the producer.
- The add-on sends `released` for the previous buffer once an EGL fence
  after its last draw has signalled.
- Pixels are premultiplied. The add-on reads the buffer as soon as it is
  shown, so the producer must have finished rendering into it.
- When the producer disconnects, the add-on drops every buffer it holds.

The shell keeps WebKit one frame ahead: it sends the next `frame` when the
previous one is `drawn`, and tells WebKit the frame was rendered at that
moment.

## Control

Control protocol version: 1

Every message is one JSON object in one packet, UTF-8.

To the add-on:

| Message | Meaning |
|---|---|
| `{"hello": 1}` | The first message: the version. |
| `{"open": "browse", "mark": PATH}` | Show the browse window: black, with the image at `PATH` (or none) in the middle until `started`, and the page over it. It is a Kodi window of its own, which Kodi leaves when playback starts. |
| `{"open": "player"}` | Show the player dialog: the page over Kodi's fullscreen video. |
| `{"close": "browse"}`, `{"close": "player"}` | Close that window. |
| `{"started": true}` | The page drew its first frame: hide the mark. |
| `{"draw": false}`, `{"draw": true}` | Stop or resume drawing the page, as while a Kodi dialog covers it. |

From the add-on:

| Message | Meaning |
|---|---|
| `{"hello": 1}` | Accepted. |
| `{"error": WHY, "hello": 1}` | Refused, with the version the add-on speaks; the connection closes. |
| `{"error": WHY, "message": MESSAGE}` | A message it couldn't handle. |
| `{"window": "browse", "id": ID, "state": "opened"}` | A window opened; `ID` is Kodi's window id. |
| `{"window": "browse", "state": "closed"}` | A window closed, by `close` or by Kodi. |
| `{"key": ACTION, "t": SECONDS, "kodi": BOOL}` | A Kodi action in an open window, with Kodi's action id, the time on `CLOCK_MONOTONIC`, and whether Kodi also acts on it. Page keys stay from Kodi, except Kodi's player keys in the player dialog; other actions (volume, screenshots) go on to Kodi. |
| `{"gone": true}` | The add-on is being unloaded; its windows are closed. |

When the control client disconnects, the add-on closes its windows, which
would otherwise keep Kodi's keys. It closes them on Kodi's GUI thread from
the next GUI frame on. The browse window waits for any modal dialog to
close first. A window Kodi opens again with no client connected, going
back through its window history, closes itself the same way.

## Unloading

The library pins itself in memory. Before Kodi unloads it, the add-on
closes its windows on Kodi's GUI thread (waiting up to 0.5 s), sends
`gone` to the control client and closes both sockets.
