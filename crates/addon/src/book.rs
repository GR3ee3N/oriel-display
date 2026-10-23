//! Track producer buffer ownership and the drawn/released replies owed for
//! each frame (docs/protocol.md, "Buffer lifecycle"). surface.rs owns GL
//! objects and reports draw and release progress to this bookkeeping state.

use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reply {
    Drawn(u32),
    Released(u32),
}

#[derive(Default)]
pub struct Book {
    /// Imported ids and whether their selected frame has been drawn.
    buffers: HashMap<u32, bool>,
    /// Buffer selected for drawing in GUI frames.
    current: Option<u32>,
    /// Superseded buffers awaiting GPU completion.
    retiring: Vec<u32>,
    /// Latest frame pending selection for drawing.
    next: Option<u32>,
}

impl Book {
    /// Queue a frame, acknowledging any superseded pending frame as drawn and
    /// released without submitting it to the GPU.
    pub fn frame(&mut self, id: u32) -> Vec<Reply> {
        match self.next.replace(id) {
            Some(old) => vec![Reply::Drawn(old), Reply::Released(old)],
            None => Vec::new(),
        }
    }

    pub fn pending(&self) -> bool {
        self.next.is_some()
    }

    /// Take the pending frame alongside the preceding buffer import requests.
    pub fn take_next(&mut self) -> Option<u32> {
        self.next.take()
    }

    /// Discard the pending frame without replying after producer disconnection.
    pub fn disconnected(&mut self) {
        self.next = None;
    }

    /// Clear buffer state without replies during producer disconnection or unload.
    pub fn clear(&mut self) {
        self.buffers.clear();
        self.current = None;
        self.retiring.clear();
    }

    pub fn imported(&mut self, id: u32) {
        self.buffers.insert(id, false);
    }

    /// Remove a buffer forgotten by the producer without issuing replies.
    pub fn forget(&mut self, id: u32) {
        self.buffers.remove(&id);
        self.retiring.retain(|&r| r != id);
        if self.current == Some(id) {
            self.current = None;
        }
    }

    /// Select the frame from take_next and retire the previous buffer.
    /// Acknowledge unknown ids immediately without drawing; these can arrive
    /// before the producer receives a reset.
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

    /// Acknowledge the first GUI draw of the selected frame.
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

    /// Release retired buffers once done confirms GPU reads have finished.
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

    /// Release held buffers when all windows close and acknowledge the pending
    /// frame as superseded before clearing imports.
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

    /// Simulate add-on bookkeeping transitions and record all replies.
    #[derive(Default)]
    struct Run {
        book: Book,
        replies: Vec<Reply>,
        /// Buffer ids whose GPU reads are still pending.
        busy: Vec<u32>,
    }

    impl Run {
        fn frame(&mut self, id: u32) {
            let replies = self.book.frame(id);
            self.replies.extend(replies);
        }

        /// Simulate one GUI frame with imports, removals, selection and drawing.
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

        /// Simulate retirement during a dirty check after GPU completion.
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

        /// Assert exactly one drawn reply followed by one released reply for each id.
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
