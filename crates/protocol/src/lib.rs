//! Wire format between a page producer (the framework's shell) and
//! vfs.oriel.display, over a `SOCK_SEQPACKET` socket in the abstract
//! namespace. Buffer fds ride along with [`kind::BUFFER`] as `SCM_RIGHTS`.
//! `docs/protocol.md` specifies it.

#![no_std]

/// The producer's socket in the abstract namespace (after the leading NUL).
pub const SOCKET: &[u8] = b"oriel-display";
/// The control client's socket: JSON messages, see docs/protocol.md.
pub const CONTROL_SOCKET: &[u8] = b"oriel-display-control";

/// The producer protocol's version, sent in [`kind::HELLO`].
pub const VERSION: u32 = 1;
/// The control protocol's version, in its `hello`.
pub const CONTROL_VERSION: u32 = 1;

/// Message kinds.
pub mod kind {
    /// Producer: a new buffer, one fd per plane attached.
    pub const BUFFER: u32 = 1;
    /// Producer: show buffer `id` from the next GUI frame.
    pub const FRAME: u32 = 2;
    /// Producer: buffer `id` is gone.
    pub const FORGET: u32 = 3;
    /// Both: the first message, `id` the protocol version. The add-on
    /// answers with the version it will use.
    pub const HELLO: u32 = 4;
    /// Add-on: buffer `id` was drawn into a GUI frame for the first time.
    pub const DRAWN: u32 = 10;
    /// Add-on: the GPU no longer reads buffer `id`.
    pub const RELEASED: u32 = 11;
    /// Add-on: the producer's version is not spoken; `id` is the add-on's.
    /// The add-on closes the connection after it.
    pub const REFUSED: u32 = 12;
    /// Add-on: the page is not drawn now; the producer may stop rendering.
    pub const HIDDEN: u32 = 13;
    /// Add-on: the page is drawn again.
    pub const SHOWN: u32 = 14;
    /// Add-on: every buffer was dropped and released (no window was open);
    /// send `BUFFER` again before showing one.
    pub const RESET: u32 = 15;
}

/// Every message, in both directions. Fields after `id` are for [`kind::BUFFER`].
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Msg {
    pub kind: u32,
    pub id: u32,
    pub width: i32,
    pub height: i32,
    /// DRM fourcc.
    pub format: u32,
    pub planes: u32,
    pub offsets: [u32; 4],
    pub strides: [u32; 4],
    /// DRM format modifier.
    pub modifier: u64,
}

pub const SIZE: usize = 64;
const _: () = assert!(size_of::<Msg>() == SIZE);

/// `DRM_FORMAT_MOD_INVALID`: no modifier given.
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
        // Little-endian targets send the struct's memory as it is.
        let memory: [u8; SIZE] = unsafe { core::mem::transmute(msg) };
        if cfg!(target_endian = "little") {
            assert_eq!(bytes, memory);
        }
    }
}
