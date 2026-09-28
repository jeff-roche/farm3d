//! P8 D4 "Capture triggers": what the Attention projector asks the camera
//! side to capture after a pass commits. The projector only yields these
//! (`attention::projector::AppliedChanges::capture`); handling them —
//! fetching the frame, storing it, and recording the outcome — is
//! `CameraServices`' job, on its own task, never awaited by the projector
//! (global constraint 5).

/// One best-effort capture a committed projector pass asked for. Yielded
/// only for `origin: live` rows, and only when the Printer's own toggle
/// (`snapshotOnIncident` / `snapshotOnCompletion`) is on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CaptureIntent {
    /// The projector opened an Incident: one `incident` frame, linked to
    /// the Incident (and its Job, if any).
    Incident {
        incident_id: String,
        printer_id: String,
        job_id: Option<String>,
    },
    /// The projector inserted `job.completed`: one `completion` frame
    /// linked to the Job, whose outcome goes to the Event's `evidence`.
    Completion {
        event_id: String,
        printer_id: String,
        job_id: String,
    },
}
