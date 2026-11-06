//! An undo history kept beside another one the player steps through with the same keys: the game's,
//! whose steps are out of our hands. Each step remembers the other history's position when it was
//! made (`at`), which says whose turn an undo or redo is, so that both undo in the order things were
//! done.
//!
//! The position counts the other history's steps: one up for each step taken or redone, one down for
//! each undone. It isn't an index into that history: the game forgets its oldest steps, so a step of
//! ours can be older than anything it can still undo, which is what `other_can` is for.

/// How many steps are kept: the oldest go first.
pub const LIMIT: usize = 100;

/// A change, and where the other history was when it was made.
#[derive(Debug)]
struct Step<T> {
    before: T,
    after: T,
    at: i64,
}

/// Changes of a `T` that can be undone and redone.
#[derive(Debug)]
pub struct History<T> {
    undo: Vec<Step<T>>,
    redo: Vec<Step<T>>,
}

impl<T> Default for History<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> History<T> {
    pub const fn new() -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    /// Adds the change from `before` to `after`, made with the other history at `at`: what was
    /// undone can't be redone any more.
    pub fn record(&mut self, before: T, after: T, at: i64) {
        self.redo.clear();
        self.undo.push(Step { before, after, at });
        if self.undo.len() > LIMIT {
            self.undo.remove(0);
        }
    }

    /// Forgets everything (another vehicle, or changes from outside).
    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }

    /// Forgets what can be redone: the other history took a new step, which ends its own redos too.
    pub fn forget_redo(&mut self) {
        self.redo.clear();
    }

    /// An undo with the other history at `position`, `other_can` undo: what to go back to if it's
    /// our turn, else `None` (the other history's). Ours is the turn when our last step is the
    /// newest, or when the other history can't go further back.
    pub fn undo(&mut self, position: i64, other_can: bool) -> Option<&T> {
        let ours = self
            .undo
            .last()
            .is_some_and(|step| step.at >= position || !other_can);
        if !ours {
            return None;
        }
        self.redo.extend(self.undo.pop());
        self.redo.last().map(|step| &step.before)
    }

    /// A redo with the other history at `position`, `other_can` redo: what to go forward to if it's
    /// our turn, else `None` (the other history's). Ours is the turn once the other history is back
    /// where our next step was made, or when it can't go further forward.
    pub fn redo(&mut self, position: i64, other_can: bool) -> Option<&T> {
        let ours = self
            .redo
            .last()
            .is_some_and(|step| step.at <= position || !other_can);
        if !ours {
            return None;
        }
        self.undo.extend(self.redo.pop());
        self.undo.last().map(|step| &step.after)
    }
}

/// The items of `to` that aren't in `from` as they are: those new or changed from one to the other.
pub fn changed<'a, T: PartialEq>(from: &[T], to: &'a [T]) -> Vec<&'a T> {
    to.iter().filter(|item| !from.contains(item)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A player using both histories: what they see, with the other history's steps as numbers.
    struct Player {
        ours: History<&'static str>,
        /// Our value now.
        value: &'static str,
        /// The other history's steps, how many are done, and the position (its steps done, plus
        /// those it forgot).
        steps: usize,
        done: usize,
        forgotten: usize,
    }

    impl Player {
        fn new() -> Self {
            Self {
                ours: History::new(),
                value: "start",
                steps: 0,
                done: 0,
                forgotten: 0,
            }
        }

        fn position(&self) -> i64 {
            (self.done + self.forgotten) as i64
        }

        fn edit_ours(&mut self, value: &'static str) {
            let before = self.value;
            self.ours.record(before, value, self.position());
            self.value = value;
        }

        fn edit_other(&mut self) {
            self.done += 1;
            self.steps = self.done;
            self.ours.forget_redo();
        }

        /// The other history forgets its oldest step, as the game does past its limits.
        fn forget_oldest(&mut self) {
            self.steps -= 1;
            self.done -= 1;
            self.forgotten += 1;
        }

        /// An undo: `true` if ours took it.
        fn undo(&mut self) -> bool {
            let position = self.position();
            match self.ours.undo(position, self.done > 0) {
                Some(value) => {
                    self.value = value;
                    true
                }
                None => {
                    self.done = self.done.saturating_sub(1);
                    false
                }
            }
        }

        /// A redo: `true` if ours took it.
        fn redo(&mut self) -> bool {
            let position = self.position();
            match self.ours.redo(position, self.done < self.steps) {
                Some(value) => {
                    self.value = value;
                    true
                }
                None => {
                    self.done = (self.done + 1).min(self.steps);
                    false
                }
            }
        }
    }

    #[test]
    fn undoes_and_redoes_a_change() {
        let mut player = Player::new();
        player.edit_ours("moved");
        assert!(player.undo());
        assert_eq!(player.value, "start");
        assert!(player.redo());
        assert_eq!(player.value, "moved");
    }

    #[test]
    fn nothing_to_undo_is_the_other_historys_turn() {
        let mut player = Player::new();
        assert!(!player.undo());
        assert!(!player.redo());
    }

    #[test]
    fn several_changes_go_back_one_at_a_time() {
        let mut player = Player::new();
        player.edit_ours("a");
        player.edit_ours("b");
        assert!(player.undo());
        assert_eq!(player.value, "a");
        assert!(player.undo());
        assert_eq!(player.value, "start");
        assert!(player.redo());
        assert_eq!(player.value, "a");
        assert!(player.redo());
        assert_eq!(player.value, "b");
    }

    #[test]
    fn both_histories_undo_and_redo_in_the_order_things_were_done() {
        let mut player = Player::new();
        player.edit_other();
        player.edit_ours("a");
        player.edit_other();
        player.edit_ours("b");

        assert!(player.undo());
        assert_eq!(player.value, "a");
        assert!(!player.undo());
        assert_eq!(player.done, 1);
        assert!(player.undo());
        assert_eq!(player.value, "start");
        assert!(!player.undo());
        assert_eq!(player.done, 0);

        assert!(!player.redo());
        assert_eq!(player.done, 1);
        assert!(player.redo());
        assert_eq!(player.value, "a");
        assert!(!player.redo());
        assert_eq!(player.done, 2);
        assert!(player.redo());
        assert_eq!(player.value, "b");
    }

    #[test]
    fn a_new_change_ends_the_redos() {
        let mut player = Player::new();
        player.edit_ours("a");
        assert!(player.undo());
        player.edit_ours("b");
        assert!(!player.redo());
        assert_eq!(player.value, "b");
    }

    #[test]
    fn a_new_step_of_the_other_history_ends_the_redos() {
        let mut player = Player::new();
        player.edit_ours("a");
        assert!(player.undo());
        player.edit_other();
        assert!(!player.redo());
        assert_eq!(player.value, "start");
    }

    #[test]
    fn a_step_older_than_what_the_other_history_remembers_still_undoes() {
        let mut player = Player::new();
        player.edit_ours("a");
        player.edit_other();
        player.edit_other();
        player.forget_oldest();

        assert!(!player.undo());
        assert_eq!(player.done, 0);
        assert!(player.undo());
        assert_eq!(player.value, "start");
        assert!(player.redo());
        assert_eq!(player.value, "a");
        assert!(!player.redo());
        assert_eq!(player.done, 1);
    }

    #[test]
    fn keeps_the_newest_steps() {
        let mut history = History::new();
        for step in 0..LIMIT + 5 {
            history.record(step, step + 1, 0);
        }
        let mut oldest = None;
        while let Some(&value) = history.undo(0, false) {
            oldest = Some(value);
        }
        assert_eq!(oldest, Some(5));
    }

    #[test]
    fn clearing_forgets_both_ways() {
        let mut history = History::new();
        history.record(1, 2, 0);
        history.record(2, 3, 0);
        assert_eq!(history.undo(0, false), Some(&2));
        history.clear();
        assert_eq!(history.undo(0, false), None);
        assert_eq!(history.redo(0, false), None);
    }

    #[test]
    fn changed_lists_what_is_new_or_different() {
        assert_eq!(changed(&[1, 2, 3], &[2, 4]), vec![&4]);
        assert!(changed(&[1, 2], &[1, 2]).is_empty());
        assert!(changed(&[1, 2], &[]).is_empty());
    }
}
