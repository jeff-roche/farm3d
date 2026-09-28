//! P8 D6 "Sinks": the notifier for every platform without one (Windows
//! and macOS in P8, decision 15). It shows nothing and reports
//! `unsupported`; the in-app Attention center works everywhere.

use async_trait::async_trait;

use super::{Notification, NotificationHandle, NotificationSink, NotifierStatus, NotifyError};

pub struct NullNotificationSink;

#[async_trait]
impl NotificationSink for NullNotificationSink {
    async fn show(&self, _notification: &Notification) -> Result<NotificationHandle, NotifyError> {
        Err(NotifyError::Unsupported)
    }

    fn status(&self) -> NotifierStatus {
        NotifierStatus::Unsupported
    }
}
