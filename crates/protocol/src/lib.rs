//! Wire format for the framework shell and vfs.oriel.display over a Linux
//! abstract SOCK_SEQPACKET socket. Attach plane descriptors to [`kind::BUFFER`]
//! packets using SCM_RIGHTS. See docs/protocol.md for the protocol contract.

#![no_std]

/// Producer socket name following the leading NUL in the Linux abstract namespace.
pub const SOCKET: &[u8] = b"oriel-display";
/// Control socket name for JSON packets described in docs/protocol.md.
pub const CONTROL_SOCKET: &[u8] = b"oriel-display-control";

/// Producer protocol version exchanged in [`kind::HELLO`].
pub const VERSION: u32 = 1;
/// Control protocol version exchanged in its JSON hello.
pub const CONTROL_VERSION: u32 = 1;

/// Producer protocol message kinds.
pub mod kind {
    /// Producer: import a buffer with one attached descriptor per plane.
    pub const BUFFER: u32 = 1;
    /// Producer: select buffer id for the next GUI frame.
    pub const FRAME: u32 = 2;
    /// Producer: remove the imported buffer id.
    pub const FORGET: u32 = 3;
    /// Both: handshake with the protocol version in id; the add-on confirms
    /// the accepted version.
    pub const HELLO: u32 = 4;
    /// Add-on: acknowledge the frame as drawn; superseded or unknown ids are acknowledged without drawing.
    pub const DRAWN: u32 = 10;
    /// Add-on: buffer id is safe for the producer to render into again.
    pub const RELEASED: u32 = 11;
    /// Add-on: refuse the protocol version, report the supported version in id,
    /// then close the connection.
    pub const REFUSED: u32 = 12;
    /// Add-on: no window draws the page; rendering may pause.
    pub const HIDDEN: u32 = 13;
    /// Add-on: a window is drawing the page.
    pub const SHOWN: u32 = 14;
    /// Add-on: all windows closed and imports were dropped; resend BUFFER
    /// before selecting any buffer again.
    pub const RESET: u32 = 15;
}

/// Fixed packet layout in both directions; fields after id apply only to [`kind::BUFFER`].
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Msg {
    pub kind: u32,
    pub id: u32,
    pub width: i32,
    pub height: i32,
    /// DRM fourcc pixel format.
    pub format: u32,
    pub planes: u32,
    pub offsets: [u32; 4],
    pub strides: [u32; 4],
    /// DRM format modifier.
    pub modifier: u64,
}

pub const SIZE: usize = 64;
const _: () = assert!(size_of::<Msg>() == SIZE);

/// DRM_FORMAT_MOD_INVALID indicates that no format modifier was supplied.
pub const MODIFIER_INVALID: u64 = 0x00ff_ffff_ffff_ffff;

impl Msg {
    pub fn new(kind: u32, id: u32) -> Msg {
        Msg { kind, id, ..Msg::default() }
    }

    pub fn to_bytes(&self) -> [u8; SIZE] {
        let mut out = [0u8; SIZE];
        let words = [self.kind, self.id, self.width as u32, self.height as u32, self.format, self.planes];
        let words = words.iter().chain(&self.offsets).chain(&self.strides);
        for (chunk, word) in out.as_chunks_mut::<4>().0.iter_mut().zip(words) {
            *chunk = word.to_le_bytes();
        }
        out[56..].copy_from_slice(&self.modifier.to_le_bytes());
        out
    }

    pub fn from_bytes(bytes: &[u8; SIZE]) -> Msg {
        let word = |i: usize| u32::from_le_bytes(bytes[i * 4..i * 4 + 4].try_into().unwrap());
        Msg {
            kind: word(0),
            id: word(1),
            width: word(2) as i32,
            height: word(3) as i32,
            format: word(4),
            planes: word(5),
            offsets: [word(6), word(7), word(8), word(9)],
            strides: [word(10), word(11), word(12), word(13)],
            modifier: u64::from_le_bytes(bytes[56..].try_into().unwrap()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::offset_of;

    #[test]
    fn layout_is_the_specified_one() {
        assert_eq!(offset_of!(Msg, kind), 0);
        assert_eq!(offset_of!(Msg, id), 4);
        assert_eq!(offset_of!(Msg, width), 8);
        assert_eq!(offset_of!(Msg, height), 12);
        assert_eq!(offset_of!(Msg, format), 16);
        assert_eq!(offset_of!(Msg, planes), 20);
        assert_eq!(offset_of!(Msg, offsets), 24);
        assert_eq!(offset_of!(Msg, strides), 40);
        assert_eq!(offset_of!(Msg, modifier), 56);
    }

    #[test]
    fn bytes_round_trip_and_match_memory() {
        let msg = Msg {
            kind: kind::BUFFER,
            id: 7,
            width: 1920,
            height: -1,
            format: 0x3432_5241,
            planes: 1,
            offsets: [1, 2, 3, 4],
            strides: [7680, 0, 0, 9],
            modifier: 0x0100_0000_0000_0001,
        };
        let bytes = msg.to_bytes();
        assert_eq!(Msg::from_bytes(&bytes), msg);
        // Little-endian targets must match the wire format byte for byte in memory.
        let memory: [u8; SIZE] = unsafe { core::mem::transmute(msg) };
        if cfg!(target_endian = "little") {
            assert_eq!(bytes, memory);
        }
    }
}
