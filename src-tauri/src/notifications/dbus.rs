//! P8 D6 "Sinks" (ADR-0015): farm3d's own `org.freedesktop.Notifications`
//! client over zbus, for Linux. The Task 2 spike
//! (`docs/superpowers/baselines/2026-09-27-p8-notification-spike.md`)
//! checked each call on KDE Plasma 6.7.5 Wayland.
//!
//! - **One connection.** A long-lived session-bus connection, opened on
//!   first use (and at startup by `refresh`). On connect the sink calls
//!   `GetServerInformation` and `GetCapabilities`; the result is the
//!   `available` status. No session bus is `noSessionBus`; no server is
//!   `noNotificationServer`; anything else that fails is `callFailed`.
//!   Nothing here panics, and every call has a timeout.
//! - **One listener.** A match rule on path
//!   `/org/freedesktop/Notifications`, interface
//!   `org.freedesktop.Notifications`, and sender = the server's current
//!   unique name, so a signal forged by another connection never arrives.
//!   The server broadcasts, so other clients' ids do arrive: the service
//!   drops ids it didn't show. Before each `Notify` the sink checks that
//!   the server's owner is unchanged, and reconnects (with a fresh rule)
//!   when it isn't.
//! - **`Notify`.** `app_name` "farm3d"; `replaces_id`; `app_icon` (the
//!   `farm3d` icon name when an installed hicolor theme has it, otherwise
//!   the bundled icon's absolute path, [`resolve_icon`]); `actions`
//!   `["default", "Open"]`; hints `desktop-entry` "farm3d" and a byte
//!   `urgency`; `expire_timeout` −1. The body is escaped when the server
//!   advertises `body-markup`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::StreamExt;
use zbus::fdo::DBusProxy;
use zbus::message::Type as MessageType;
use zbus::names::BusName;
use zbus::zvariant::Value;
use zbus::{Connection, MatchRule, MessageStream};

use super::{
    escape_body_markup, Notification, NotificationHandle, NotificationSink, NotifierStatus,
    NotifierUnavailableReason, NotifyError, SignalHandler, SinkSignal,
};

const PATH: &str = "/org/freedesktop/Notifications";
const INTERFACE: &str = "org.freedesktop.Notifications";
const SERVICE: &str = "org.freedesktop.Notifications";
/// Every D-Bus call's limit.
const CALL_TIMEOUT: Duration = Duration::from_secs(5);
/// The installed icon's name (the deb's `hicolor/*/apps/farm3d.png`).
pub const ICON_NAME: &str = "farm3d";

#[zbus::proxy(
    interface = "org.freedesktop.Notifications",
    default_service = "org.freedesktop.Notifications",
    default_path = "/org/freedesktop/Notifications"
)]
trait Notifications {
    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        app_name: &str,
        replaces_id: u32,
        app_icon: &str,
        summary: &str,
        body: &str,
        actions: &[&str],
        hints: HashMap<&str, Value<'_>>,
        expire_timeout: i32,
    ) -> zbus::Result<u32>;
    fn get_capabilities(&self) -> zbus::Result<Vec<String>>;
    fn get_server_information(&self) -> zbus::Result<(String, String, String, String)>;
}

/// D6's `app_icon`: [`ICON_NAME`] when an icon theme under one of
/// `data_dirs` has `icons/hicolor/<size>/apps/farm3d.png`, otherwise the
/// bundled icon's absolute path, otherwise none.
pub fn resolve_icon(data_dirs: &[PathBuf], bundled: Option<&Path>) -> String {
    let installed = data_dirs.iter().any(|dir| {
        std::fs::read_dir(dir.join("icons/hicolor"))
            .map(|sizes| {
                sizes.flatten().any(|size| {
                    size.path()
                        .join("apps")
                        .join(format!("{ICON_NAME}.png"))
                        .is_file()
                })
            })
            .unwrap_or(false)
    });
    if installed {
        return ICON_NAME.to_string();
    }
    bundled
        .filter(|path| path.is_file())
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// `$XDG_DATA_HOME` (default `~/.local/share`), then `$XDG_DATA_DIRS`
/// (default `/usr/local/share:/usr/share`).
pub fn xdg_data_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    match std::env::var_os("XDG_DATA_HOME").filter(|value| !value.is_empty()) {
        Some(home) => dirs.push(PathBuf::from(home)),
        None => {
            if let Some(home) = std::env::var_os("HOME") {
                dirs.push(PathBuf::from(home).join(".local/share"));
            }
        }
    }
    let system = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".to_string());
    dirs.extend(system.split(':').filter(|dir| !dir.is_empty()).map(PathBuf::from));
    dirs
}

/// The live connection and its listener.
struct Live {
    connection: Connection,
    proxy: NotificationsProxy<'static>,
    owner: String,
    body_markup: bool,
    /// Set when the listener's stream ended: the next call reconnects.
    stale: std::sync::Arc<AtomicBool>,
    listener: tauri::async_runtime::JoinHandle<()>,
}

impl Drop for Live {
    fn drop(&mut self) {
        self.listener.abort();
    }
}

/// farm3d's `org.freedesktop.Notifications` client.
pub struct DbusNotificationSink {
    icon: String,
    live: tokio::sync::Mutex<Option<Live>>,
    status: Mutex<NotifierStatus>,
    handler: std::sync::Arc<Mutex<Option<SignalHandler>>>,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

async fn bounded<T>(
    call: impl std::future::Future<Output = zbus::Result<T>>,
) -> Result<T, NotifierUnavailableReason> {
    match tokio::time::timeout(CALL_TIMEOUT, call).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(classify(&error)),
        Err(_) => Err(NotifierUnavailableReason::CallFailed),
    }
}

/// A call to a name nobody owns (and nobody can activate) is "no server".
fn classify(error: &zbus::Error) -> NotifierUnavailableReason {
    let no_server = |name: &str| {
        name == "org.freedesktop.DBus.Error.ServiceUnknown"
            || name == "org.freedesktop.DBus.Error.NameHasNoOwner"
    };
    match error {
        zbus::Error::MethodError(name, _, _) if no_server(name.as_str()) => {
            NotifierUnavailableReason::NoNotificationServer
        }
        zbus::Error::FDO(fdo) => match fdo.as_ref() {
            zbus::fdo::Error::ServiceUnknown(_) | zbus::fdo::Error::NameHasNoOwner(_) => {
                NotifierUnavailableReason::NoNotificationServer
            }
            _ => NotifierUnavailableReason::CallFailed,
        },
        _ => NotifierUnavailableReason::CallFailed,
    }
}

async fn current_owner(connection: &Connection) -> Result<String, NotifierUnavailableReason> {
    let dbus = bounded(DBusProxy::new(connection)).await?;
    let name = BusName::try_from(SERVICE).map_err(|_| NotifierUnavailableReason::CallFailed)?;
    match tokio::time::timeout(CALL_TIMEOUT, dbus.get_name_owner(name)).await {
        Ok(Ok(owner)) => Ok(owner.to_string()),
        Ok(Err(_)) => Err(NotifierUnavailableReason::NoNotificationServer),
        Err(_) => Err(NotifierUnavailableReason::CallFailed),
    }
}

/// The listener: every signal message becomes a [`SinkSignal`] for the
/// handler. A message the stream fails to read is skipped; when the stream
/// ends (the connection dropped), `stale` is set so the next call
/// reconnects instead of showing notifications no one listens for.
async fn listen<S>(
    mut stream: S,
    handler: std::sync::Arc<Mutex<Option<SignalHandler>>>,
    stale: std::sync::Arc<AtomicBool>,
) where
    S: futures_util::Stream<Item = zbus::Result<zbus::Message>> + Unpin,
{
    while let Some(received) = stream.next().await {
        let Ok(message) = received else {
            continue;
        };
        let Some(signal) = parse_signal(&message) else {
            continue;
        };
        let handler = lock(&handler).clone();
        if let Some(handler) = handler {
            handler(signal);
        }
    }
    stale.store(true, Ordering::SeqCst);
}

/// One signal message as a [`SinkSignal`]; `None` for anything else.
fn parse_signal(message: &zbus::Message) -> Option<SinkSignal> {
    let header = message.header();
    let member = header.member()?.to_string();
    let body = message.body();
    match member.as_str() {
        "ActionInvoked" => body
            .deserialize::<(u32, String)>()
            .ok()
            .map(|(id, action)| SinkSignal::ActionInvoked { id, action }),
        "ActivationToken" => body
            .deserialize::<(u32, String)>()
            .ok()
            .map(|(id, token)| SinkSignal::ActivationToken { id, token }),
        "NotificationClosed" => body
            .deserialize::<(u32, u32)>()
            .ok()
            .map(|(id, reason)| SinkSignal::Closed { id, reason }),
        _ => None,
    }
}

impl DbusNotificationSink {
    /// Doesn't connect: the first `refresh` or `show` does.
    pub fn new(icon: String) -> Self {
        Self {
            icon,
            live: tokio::sync::Mutex::new(None),
            status: Mutex::new(NotifierStatus::Unavailable {
                reason: NotifierUnavailableReason::CallFailed,
            }),
            handler: std::sync::Arc::new(Mutex::new(None)),
        }
    }

    fn set_status(&self, status: NotifierStatus) -> NotifierStatus {
        *lock(&self.status) = status.clone();
        status
    }

    async fn connect(&self) -> Result<Live, NotifierUnavailableReason> {
        let connection = match tokio::time::timeout(CALL_TIMEOUT, Connection::session()).await {
            Ok(Ok(connection)) => connection,
            Ok(Err(_)) => return Err(NotifierUnavailableReason::NoSessionBus),
            Err(_) => return Err(NotifierUnavailableReason::NoSessionBus),
        };
        let proxy = bounded(NotificationsProxy::new(&connection)).await?;
        // Activates the server if it is D-Bus activatable.
        let (name, vendor, version, spec_version) =
            bounded(proxy.get_server_information()).await?;
        let capabilities = bounded(proxy.get_capabilities()).await?;
        let owner = current_owner(&connection).await?;
        let rule = MatchRule::builder()
            .msg_type(MessageType::Signal)
            .sender(owner.as_str())
            .and_then(|rule| rule.path(PATH))
            .and_then(|rule| rule.interface(INTERFACE))
            .map_err(|_| NotifierUnavailableReason::CallFailed)?
            .build();
        let stream = bounded(MessageStream::for_match_rule(rule, &connection, Some(64))).await?;
        let stale = std::sync::Arc::new(AtomicBool::new(false));
        let listener = tauri::async_runtime::spawn(listen(
            stream,
            std::sync::Arc::clone(&self.handler),
            std::sync::Arc::clone(&stale),
        ));
        let has = |capability: &str| capabilities.iter().any(|c| c == capability);
        self.set_status(NotifierStatus::Available {
            server_name: name,
            server_vendor: vendor,
            server_version: version,
            spec_version,
            actions: has("actions"),
            body_markup: has("body-markup"),
        });
        Ok(Live {
            connection,
            proxy,
            owner,
            body_markup: has("body-markup"),
            stale,
            listener,
        })
    }

    /// The live connection, reconnecting when there is none or the
    /// server's owner changed (a restarted server has a new unique name,
    /// and the listener's rule would miss its signals).
    async fn ensure_live<'a>(
        &self,
        live: &'a mut Option<Live>,
    ) -> Result<&'a Live, NotifierUnavailableReason> {
        if live
            .as_ref()
            .is_some_and(|current| current.stale.load(Ordering::SeqCst))
        {
            *live = None;
        }
        if let Some(current) = live.as_ref() {
            match current_owner(&current.connection).await {
                Ok(owner) if owner == current.owner => {}
                _ => *live = None,
            }
        }
        if live.is_none() {
            match self.connect().await {
                Ok(connected) => *live = Some(connected),
                Err(reason) => {
                    self.set_status(NotifierStatus::Unavailable { reason });
                    return Err(reason);
                }
            }
        }
        Ok(live.as_ref().expect("connected above"))
    }
}

#[async_trait]
impl NotificationSink for DbusNotificationSink {
    async fn show(&self, notification: &Notification) -> Result<NotificationHandle, NotifyError> {
        let mut live = self.live.lock().await;
        let connected = self
            .ensure_live(&mut live)
            .await
            .map_err(NotifyError::Unavailable)?;
        let body = if connected.body_markup {
            escape_body_markup(&notification.body)
        } else {
            notification.body.clone()
        };
        let hints = HashMap::from([
            ("desktop-entry", Value::from("farm3d")),
            ("urgency", Value::U8(notification.urgency.byte())),
        ]);
        let shown = bounded(connected.proxy.notify(
            "farm3d",
            notification.replaces_id,
            &self.icon,
            &notification.summary,
            &body,
            &["default", "Open"],
            hints,
            -1,
        ))
        .await;
        match shown {
            Ok(id) => Ok(NotificationHandle { id }),
            Err(reason) => {
                // Reconnect on the next call.
                *live = None;
                self.set_status(NotifierStatus::Unavailable { reason });
                Err(NotifyError::Unavailable(reason))
            }
        }
    }

    fn status(&self) -> NotifierStatus {
        lock(&self.status).clone()
    }

    async fn refresh(&self) -> NotifierStatus {
        let mut live = self.live.lock().await;
        match self.ensure_live(&mut live).await {
            Ok(_) => self.status(),
            Err(reason) => NotifierStatus::Unavailable { reason },
        }
    }

    fn set_signal_handler(&self, handler: SignalHandler) {
        *lock(&self.handler) = Some(handler);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_icon_is_the_installed_name_else_the_bundled_path_else_none() {
        let temp = tempfile::tempdir().unwrap();
        let installed = temp.path().join("installed");
        let empty = temp.path().join("empty");
        std::fs::create_dir_all(installed.join("icons/hicolor/128x128/apps")).unwrap();
        std::fs::write(installed.join("icons/hicolor/128x128/apps/farm3d.png"), b"png").unwrap();
        std::fs::create_dir_all(empty.join("icons/hicolor/128x128/apps")).unwrap();
        let bundled = temp.path().join("farm3d-notification.png");
        std::fs::write(&bundled, b"png").unwrap();

        assert_eq!(
            resolve_icon(&[empty.clone(), installed.clone()], Some(&bundled)),
            "farm3d"
        );
        assert_eq!(
            resolve_icon(&[empty.clone()], Some(&bundled)),
            bundled.to_string_lossy()
        );
        assert_eq!(
            resolve_icon(&[empty], Some(&temp.path().join("missing.png"))),
            ""
        );
    }

    #[test]
    fn a_missing_server_is_no_notification_server_and_the_rest_call_failed() {
        let unknown = zbus::Error::FDO(Box::new(zbus::fdo::Error::ServiceUnknown("x".into())));
        assert_eq!(
            classify(&unknown),
            NotifierUnavailableReason::NoNotificationServer
        );
        assert_eq!(
            classify(&zbus::Error::InvalidReply),
            NotifierUnavailableReason::CallFailed
        );
    }

    /// No session bus address and no runtime dir: `noSessionBus`, never a
    /// panic. Run in a child process so the environment change can't leak
    /// into other tests.
    #[test]
    fn without_a_session_bus_the_status_is_no_session_bus() {
        if std::env::var_os("FARM3D_DBUS_NO_BUS_CHILD").is_some() {
            let sink = DbusNotificationSink::new(String::new());
            let status = tauri::async_runtime::block_on(sink.refresh());
            assert_eq!(
                status,
                NotifierStatus::Unavailable {
                    reason: NotifierUnavailableReason::NoSessionBus
                }
            );
            let shown = tauri::async_runtime::block_on(sink.show(&super::super::Notification::test()));
            assert_eq!(
                shown,
                Err(NotifyError::Unavailable(NotifierUnavailableReason::NoSessionBus))
            );
            return;
        }
        let empty = tempfile::tempdir().unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "notifications::dbus::tests::without_a_session_bus_the_status_is_no_session_bus",
                "--nocapture",
            ])
            .env("FARM3D_DBUS_NO_BUS_CHILD", "1")
            .env_remove("DBUS_SESSION_BUS_ADDRESS")
            .env("XDG_RUNTIME_DIR", empty.path())
            .status()
            .unwrap();
        assert!(status.success());
    }

    /// A stream error is skipped, not the end of the listener; the end of
    /// the stream marks the connection stale.
    #[test]
    fn the_listener_skips_a_stream_error_and_marks_the_connection_stale_at_the_end() {
        let signal = |member: &str, body: &(u32, String)| {
            zbus::Message::signal(PATH, INTERFACE, member)
                .unwrap()
                .build(body)
                .unwrap()
        };
        let messages: Vec<zbus::Result<zbus::Message>> = vec![
            Ok(signal("ActivationToken", &(7, "token-7".to_string()))),
            Err(zbus::Error::InvalidReply),
            Ok(signal("ActionInvoked", &(7, "default".to_string()))),
        ];
        let seen = std::sync::Arc::new(Mutex::new(Vec::new()));
        let recorded = std::sync::Arc::clone(&seen);
        let handler: SignalHandler = std::sync::Arc::new(move |signal| {
            recorded.lock().unwrap().push(signal);
        });
        let stale = std::sync::Arc::new(AtomicBool::new(false));
        tauri::async_runtime::block_on(listen(
            futures_util::stream::iter(messages),
            std::sync::Arc::new(Mutex::new(Some(handler))),
            std::sync::Arc::clone(&stale),
        ));
        assert_eq!(
            *seen.lock().unwrap(),
            vec![
                SinkSignal::ActivationToken {
                    id: 7,
                    token: "token-7".into()
                },
                SinkSignal::ActionInvoked {
                    id: 7,
                    action: "default".into()
                },
            ]
        );
        assert!(stale.load(Ordering::SeqCst));
    }
}
