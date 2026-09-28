//! P8 D6 "Policy": the input the notification policy decides on. The
//! Attention projector yields one [`NotifyCandidate`] per Event it
//! inserted with `origin: live` (`attention::projector::AppliedChanges::
//! notify`); amendments, acknowledgements, resolutions, and backfilled
//! inserts never become candidates. Whether a candidate is shown (its
//! class setting, the Printer's muting, focus, and the rate limiter) is
//! `decide`'s concern, beside this type.

use crate::attention::AttentionEvent;

/// An Event the projector just inserted, live.
#[derive(Clone, Debug, PartialEq)]
pub struct NotifyCandidate {
    pub event: AttentionEvent,
    /// Whether it recurred (`recurrenceOf` set): a recurrence notifies as
    /// a new Event does.
    pub recurred: bool,
}
