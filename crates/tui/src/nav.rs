//! Where `Esc` goes: the screens left behind, most recent last.

/// How many screens the stack remembers.
pub const DEPTH: usize = 32;

/// The screens the user came from, like a browser's history: going
/// somewhere pushes the screen being left, going back pops it. It keeps the
/// last [`DEPTH`] and forgets older ones.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackStack<T> {
    stack: Vec<T>,
}

impl<T> Default for BackStack<T> {
    fn default() -> Self {
        BackStack { stack: Vec::new() }
    }
}

impl<T: Copy + PartialEq> BackStack<T> {
    /// The user goes from `from` to `to`.
    pub fn visit(&mut self, from: T, to: T) {
        if from == to {
            return;
        }
        if self.stack.len() == DEPTH {
            self.stack.remove(0);
        }
        self.stack.push(from);
    }

    /// The screen `Esc` returns to, taking it off the stack.
    pub fn back(&mut self) -> Option<T> {
        self.stack.pop()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.stack.is_empty()
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.stack.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn esc_walks_back_through_the_screens_visited() {
        let mut s = BackStack::default();
        s.visit(1, 4);
        s.visit(4, 5);
        s.visit(5, 0);
        assert_eq!(s.back(), Some(5));
        assert_eq!(s.back(), Some(4));
        assert_eq!(s.back(), Some(1));
        assert_eq!(s.back(), None);
        assert!(s.is_empty());
    }

    #[test]
    fn staying_put_adds_nothing() {
        let mut s = BackStack::default();
        s.visit(2, 2);
        assert!(s.is_empty());
    }

    #[test]
    fn going_back_and_forth_is_remembered_like_a_history() {
        let mut s = BackStack::default();
        s.visit(1, 4);
        s.visit(4, 1);
        assert_eq!(s.back(), Some(4), "esc from 1 returns to 4, where it was");
        assert_eq!(s.back(), Some(1));
        assert_eq!(s.back(), None);
    }

    #[test]
    fn the_stack_forgets_the_oldest_past_its_depth() {
        let mut s = BackStack::default();
        let mut at = 0u32;
        for to in 1..=(DEPTH as u32 + 5) {
            s.visit(at, to);
            at = to;
        }
        assert_eq!(s.len(), DEPTH);
        let mut seen = Vec::new();
        while let Some(x) = s.back() {
            seen.push(x);
        }
        assert_eq!(seen.first(), Some(&(DEPTH as u32 + 4)));
        assert_eq!(seen.last(), Some(&5), "the first five were forgotten");
    }
}
