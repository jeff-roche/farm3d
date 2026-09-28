//! P8 D6 "Focus": whether farm3d's main window has focus.
//!
//! Seeded once from the main window's `is_focused()` right after the
//! window is shown during setup ([`Focus::seed`], decision 41), then fed
//! only by its `WindowEvent::Focused(f)` (`lib.rs`). The seed covers a
//! window the compositor opened unfocused without sending
//! `Focused(false)` (Task 2), which the old fixed `true` start kept silent
//! until the operator focused and left it once. If `is_focused()` errors
//! (tauri#11323 territory), the seed is `false`: notify rather than stay
//! silent. The events stay the source after that.
//!
//! It also counts every `Focused(true)`, so a click's raise can tell
//! whether the window came forward after it asked (D6 "Click
//! activation", step 3).

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;

#[derive(Clone, Copy, Debug)]
struct FocusState {
    focused: bool,
    /// How many `Focused(true)` events have arrived.
    gained: u64,
}

/// The main window's focus. Cheap to clone; every clone is the same state.
#[derive(Clone, Debug)]
pub struct Focus {
    state: Arc<watch::Sender<FocusState>>,
}

impl Default for Focus {
    fn default() -> Self {
        Self {
            state: Arc::new(
                watch::channel(FocusState {
                    focused: true,
                    gained: 0,
                })
                .0,
            ),
        }
    }
}

impl Focus {
    /// Decision 41: the one seed at launch, from the shown main window's
    /// `is_focused()` (`observed`). An error, or no window to ask, seeds
    /// `false`. Not a `Focused(true)` event, so [`Focus::gained`] is
    /// unchanged; any event after it wins.
    pub fn seed<E>(&self, observed: Result<bool, E>) {
        let focused = observed.unwrap_or(false);
        self.state.send_modify(|state| state.focused = focused);
    }

    /// One `WindowEvent::Focused(focused)`.
    pub fn set(&self, focused: bool) {
        self.state.send_modify(|state| {
            state.focused = focused;
            if focused {
                state.gained += 1;
            }
        });
    }

    pub fn is_focused(&self) -> bool {
        self.state.borrow().focused
    }

    /// How many `Focused(true)` events have arrived so far: a mark for
    /// [`Focus::regained_since`].
    pub fn gained(&self) -> u64 {
        self.state.borrow().gained
    }

    /// Waits up to `within` for a `Focused(true)` after `mark`. `true` at
    /// once when one already arrived, or when the window is focused now
    /// (a click on a notification while farm3d had focus sends none).
    pub async fn regained_since(&self, mark: u64, within: Duration) -> bool {
        let mut receiver = self.state.subscribe();
        let done = |state: &FocusState| state.gained > mark || state.focused;
        if done(&receiver.borrow()) {
            return true;
        }
        tokio::time::timeout(within, receiver.wait_for(done))
            .await
            .is_ok_and(|waited| waited.is_ok())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_starts_focused_and_counts_each_focus_gained() {
        let focus = Focus::default();
        assert!(focus.is_focused());
        assert_eq!(focus.gained(), 0);
        focus.set(false);
        assert!(!focus.is_focused());
        focus.set(true);
        focus.set(true);
        assert_eq!(focus.gained(), 2);
    }

    #[test]
    fn the_seed_takes_is_focused_and_an_error_seeds_unfocused() {
        let focus = Focus::default();
        focus.seed(Ok::<_, ()>(false));
        assert!(!focus.is_focused(), "a window shown unfocused notifies at once");
        focus.seed(Ok::<_, ()>(true));
        assert!(focus.is_focused());
        assert_eq!(focus.gained(), 0, "a seed is not a Focused(true) event");

        let errored = Focus::default();
        errored.seed(Err::<bool, _>("is_focused failed"));
        assert!(!errored.is_focused(), "an error seeds false: notify, not silence");

        // The events stay the source after the seed.
        errored.set(true);
        assert!(errored.is_focused());
        assert_eq!(errored.gained(), 1);
    }

    #[tokio::test]
    async fn regained_since_waits_for_a_later_focus_or_times_out() {
        let focus = Focus::default();
        focus.set(false);
        let mark = focus.gained();
        assert!(!focus.regained_since(mark, Duration::from_millis(20)).await);
        let later = focus.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            later.set(true);
        });
        assert!(focus.regained_since(mark, Duration::from_secs(5)).await);
    }
}
