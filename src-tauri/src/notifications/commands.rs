//! P8 D7: `notification_status` and `send_test_notification`.

use serde::Serialize;
use ts_rs::TS;

use crate::bootstrap::BootstrapState;
use crate::contracts::command::{CommandError, CommandSuccess, IncomingContractVersion};
use crate::RuntimeServices;

use super::NotifierStatus;

type Services<'a, R> = tauri::State<'a, BootstrapState<RuntimeServices<R>>>;

/// `send_test_notification`'s result.
#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/TestNotificationSent.ts")]
pub struct TestNotificationSent {
    #[ts(type = "true")]
    pub sent: bool,
}

/// `notification_status`: whether desktop notifications can be shown here.
/// A notifier that isn't `available` is asked again first (D6: the service
/// retries the connection on the next status call).
#[tauri::command]
pub async fn notification_status<R: tauri::Runtime>(
    _app: tauri::AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
) -> Result<CommandSuccess<NotifierStatus>, CommandError> {
    contract_version.validate()?;
    let services = bootstrap.ready()?;
    Ok(CommandSuccess::new(services.notifications.status().await))
}

/// `send_test_notification`: one notification whatever the focus and
/// classes ("farm3d test notification", targeting the Monitor).
/// `NOTIFICATIONS_UNAVAILABLE` with the notifier's status otherwise.
#[tauri::command]
pub async fn send_test_notification<R: tauri::Runtime>(
    _app: tauri::AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
) -> Result<CommandSuccess<TestNotificationSent>, CommandError> {
    contract_version.validate()?;
    let services = bootstrap.ready()?;
    services
        .notifications
        .send_test()
        .await
        .map(|_| CommandSuccess::new(TestNotificationSent { sent: true }))
        .map_err(|status| CommandError::notifications_unavailable(&status))
}
