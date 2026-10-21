//! Which of the producer's buffers the add-on holds, and the `drawn` and
//! `released` replies it owes for them (docs/protocol.md, "Rules"): each
//! `frame` gets one of each. The book holds no GL objects; surface.rs does
//! the GL work and tells it what happened.

use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reply {
    Drawn(u32),
    Released(u32),
}

#[derive(Default)]
pub struct Book {
    /// The imported buffers, and whether each was drawn since its `frame`.
    buffers: HashMap<u32, bool>,
    /// The buffer the GUI frames draw.
    current: Option<u32>,
    /// Buffers no longer shown, which the GPU may still read.
    retiring: Vec<u32>,
    /// The newest `frame`, not shown yet.
    next: Option<u32>,
}

impl Book {
    /// A `frame` from the producer. One that came before it and was never
    /// shown is replaced: never drawn, never read by the GPU.
    pub fn frame(&mut self, id: u32) -> Vec<Reply> {
        match self.next.replace(id) {
            Some(old) => vec![Reply::Drawn(old), Reply::Released(old)],
            None => Vec::new(),
        }
    }

    pub fn pending(&self) -> bool {
        self.next.is_some()
    }

    /// The frame to show, taken together with the buffers sent before it.
    pub fn take_next(&mut self) -> Option<u32> {
        self.next.take()
    }

    /// The producer disconnected: its pending frame needs no answer.
    pub fn disconnected(&mut self) {
        self.next = None;
    }

    /// Forgets every buffer without replies, for a producer that is gone or
    /// an add-on that is unloading.
    pub fn clear(&mut self) {
        self.buffers.clear();
        self.current = None;
        self.retiring.clear();
    }

    pub fn imported(&mut self, id: u32) {
        self.buffers.insert(id, false);
    }

    /// The producer dropped the buffer.
    pub fn forget(&mut self, id: u32) {
        self.buffers.remove(&id);
        self.retiring.retain(|&r| r != id);
        if self.current == Some(id) {
            self.current = None;
        }
    }

    /// Shows the frame taken with `take_next`. The buffer shown before goes
    /// to retiring. A frame for a buffer the add-on doesn't have (sent before
    /// a reset reached the producer) is never drawn: it goes straight back.
    pub fn show(&mut self, next: Option<u32>) -> Vec<Reply> {
        let Some(next) = next else { return Vec::new() };
        let Some(drawn) = self.buffers.get_mut(&next) else {
            return vec![Reply::Drawn(next), Reply::Released(next)];
        };
        if self.current != Some(next) {
            *drawn = false;
            if let Some(current) = self.current {
                self.retiring.push(current);
            }
            self.current = Some(next);
        }
        Vec::new()
    }

    pub fn current(&self) -> Option<u32> {
        self.current
    }

    /// The current buffer went into a GUI frame: `drawn`, the first time.
    pub fn drew(&mut self) -> Vec<Reply> {
        let Some(id) = self.current else { return Vec::new() };
        match self.buffers.get_mut(&id) {
            Some(drawn) if !*drawn => {
                *drawn = true;
                vec![Reply::Drawn(id)]
            }
            _ => Vec::new(),
        }
    }

    /// Releases the retiring buffers for which `done` says the GPU no longer
    /// reads them.
    pub fn retire(&mut self, mut done: impl FnMut(u32) -> bool) -> Vec<Reply> {
        let mut replies = Vec::new();
        self.retiring.retain(|&id| {
            if !done(id) {
                return true;
            }
            replies.push(Reply::Released(id));
            false
        });
        replies
    }

    /// No window is open: every buffer goes back to the producer, and a
    /// frame not shown yet is answered as if replaced.
    pub fn drop_buffers(&mut self) -> Vec<Reply> {
        let held = self.current.take().into_iter().chain(self.retiring.drain(..));
        let mut replies: Vec<Reply> = held.map(Reply::Released).collect();
        if let Some(id) = self.next.take() {
            replies.extend([Reply::Drawn(id), Reply::Released(id)]);
        }
        self.buffers.clear();
        replies
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A book driven the way the add-on drives it, recording every reply.
    #[derive(Default)]
    struct Run {
        book: Book,
        replies: Vec<Reply>,
        /// Buffers the GPU still reads.
        busy: Vec<u32>,
    }

    impl Run {
        fn frame(&mut self, id: u32) {
            let replies = self.book.frame(id);
            self.replies.extend(replies);
        }

        /// One GUI frame: take what the producer sent, show it and draw it.
        fn render(&mut self, imported: &[u32], forget: &[u32], reset: bool) {
            let next = self.book.take_next();
            if reset {
                self.book.clear();
            }
            for &id in imported {
                self.book.imported(id);
            }
            for &id in forget {
                self.book.forget(id);
            }
            let replies = self.book.show(next);
            self.replies.extend(replies);
            let replies = self.book.drew();
            self.replies.extend(replies);
            self.busy.extend(self.book.current());
        }

        /// The next frame's dirty check, with the GPU done.
        fn retire(&mut self) {
            self.busy.clear();
            let busy = &self.busy;
            let replies = self.book.retire(|id| !busy.contains(&id));
            self.replies.extend(replies);
        }

        fn drop_buffers(&mut self) {
            let replies = self.book.drop_buffers();
            self.replies.extend(replies);
        }

        fn count(&self, reply: Reply) -> usize {
            self.replies.iter().filter(|&&r| r == reply).count()
        }

        /// Each of `ids`' frames got one `drawn`, then one `released`.
        fn answered(&self, ids: &[u32]) {
            for &id in ids {
                assert_eq!(self.count(Reply::Drawn(id)), 1, "drawn {id}: {:?}", self.replies);
                assert_eq!(self.count(Reply::Released(id)), 1, "released {id}: {:?}", self.replies);
                let drawn = self.replies.iter().position(|&r| r == Reply::Drawn(id));
                let released = self.replies.iter().position(|&r| r == Reply::Released(id));
                assert!(drawn < released, "{id}: {:?}", self.replies);
            }
            assert_eq!(self.replies.len(), 2 * ids.len(), "{:?}", self.replies);
        }
    }

    #[test]
    fn shown_frames_are_drawn_then_released() {
        let mut run = Run::default();
        run.frame(1);
        run.render(&[1, 2], &[], false);
        assert_eq!(run.replies, [Reply::Drawn(1)]);
        run.frame(2);
        run.render(&[], &[], false);
        assert_eq!(run.replies, [Reply::Drawn(1), Reply::Drawn(2)]);
        run.retire();
        run.frame(1);
        run.render(&[], &[], false);
        run.retire();
        assert_eq!(
            run.replies,
            [Reply::Drawn(1), Reply::Drawn(2), Reply::Released(1), Reply::Drawn(1), Reply::Released(2)]
        );
    }

    #[test]
    fn retiring_waits_for_the_gpu() {
        let mut run = Run::default();
        run.frame(1);
        run.render(&[1, 2], &[], false);
        run.frame(2);
        run.render(&[], &[], false);
        let replies = run.book.retire(|_| false);
        assert!(replies.is_empty());
        run.retire();
        run.drop_buffers();
        run.answered(&[1, 2]);
    }

    #[test]
    fn a_replaced_frame_is_drawn_and_released_at_once() {
        let mut run = Run::default();
        run.frame(1);
        run.frame(2);
        assert_eq!(run.replies, [Reply::Drawn(1), Reply::Released(1)]);
        run.render(&[1, 2], &[], false);
        run.drop_buffers();
        run.answered(&[1, 2]);
    }

    #[test]
    fn a_frame_for_an_unknown_buffer_goes_straight_back() {
        let mut run = Run::default();
        run.frame(7);
        run.render(&[1], &[], false);
        assert_eq!(run.replies, [Reply::Drawn(7), Reply::Released(7)]);
        assert_eq!(run.book.current(), None);
        run.answered(&[7]);
    }

    #[test]
    fn a_forgotten_buffer_is_unknown() {
        let mut run = Run::default();
        run.render(&[1, 2], &[], false);
        run.render(&[], &[1], false);
        run.frame(1);
        run.render(&[], &[], false);
        run.answered(&[1]);
    }

    #[test]
    fn forget_drops_a_held_buffer_without_replies() {
        let mut run = Run::default();
        run.frame(1);
        run.render(&[1, 2], &[], false);
        run.frame(2);
        run.render(&[], &[1], false);
        run.retire();
        assert_eq!(run.replies, [Reply::Drawn(1), Reply::Drawn(2)]);
        assert_eq!(run.book.current(), Some(2));
    }

    #[test]
    fn a_reset_drops_everything_without_replies() {
        let mut run = Run::default();
        run.frame(1);
        run.render(&[1, 2], &[], false);
        run.frame(2);
        run.render(&[], &[], false);
        run.book.disconnected();
        run.render(&[], &[], true);
        run.retire();
        assert_eq!(run.book.current(), None);
        assert_eq!(run.replies, [Reply::Drawn(1), Reply::Drawn(2)]);
    }

    #[test]
    fn a_frame_after_a_reset_needs_its_buffer_again() {
        let mut run = Run::default();
        run.render(&[1], &[], false);
        run.frame(1);
        run.render(&[3], &[], true);
        run.answered(&[1]);
        run.replies.clear();
        run.frame(3);
        run.render(&[], &[], false);
        run.drop_buffers();
        run.answered(&[3]);
    }

    #[test]
    fn drop_buffers_answers_held_and_pending_frames() {
        let mut run = Run::default();
        run.frame(1);
        run.render(&[1, 2, 3], &[], false);
        run.frame(2);
        run.render(&[], &[], false);
        run.frame(3);
        run.drop_buffers();
        run.answered(&[1, 2, 3]);
        assert!(!run.book.pending());
        assert_eq!(run.book.current(), None);
        run.frame(1);
        run.render(&[], &[], false);
        assert_eq!(run.replies[6..], [Reply::Drawn(1), Reply::Released(1)]);
    }
}
