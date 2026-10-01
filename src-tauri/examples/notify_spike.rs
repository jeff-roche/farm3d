//! P8 Task 2: the Linux notification and focus spike (spec D6, ADR-0015).
//!
//! Throwaway. It checks, on a live desktop session, the parts of the
//! D-Bus notification design that don't need a person, and gives Task 17
//! (the owner at the desktop) a way to check the parts that do.
//!
//! ```sh
//! cargo run --manifest-path src-tauri/Cargo.toml --example notify_spike
//! cargo run --manifest-path src-tauri/Cargo.toml --example notify_spike -- --focus-only
//! cargo run --manifest-path src-tauri/Cargo.toml --example notify_spike -- --click
//! ```
//!
//! The default mode sends three test notifications (and one in-place
//! replacement), closes every one of them itself, then scripts the
//! window through hide/show/minimize/unminimize/set_focus and logs which
//! `WindowEvent::Focused` events arrive (`--focus-only` runs just the
//! window part, and sends nothing). `--click` minimizes the window,
//! sends one notification, and waits up to two minutes for someone to
//! click it, then runs D6's click-activation sequence and logs each step.
//! See `docs/superpowers/baselines/2026-09-27-p8-notification-spike.md`.

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("notify_spike is Linux-only (ADR-0015)");
}

#[cfg(target_os = "linux")]
fn main() {
    spike::run();
}

#[cfg(target_os = "linux")]
mod spike {
    use std::collections::{HashMap, HashSet};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex, OnceLock};
    use std::time::{Duration, Instant};

    use futures_util::StreamExt;
    use gtk::prelude::GtkWindowExt;
    use tauri::{
        AppHandle, UserAttentionType, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent,
    };
    use zbus::fdo::DBusProxy;
    use zbus::message::Type as MessageType;
    use zbus::names::BusName;
    use zbus::zvariant::Value;
    use zbus::{Connection, MatchRule, MessageStream};

    const PATH: &str = "/org/freedesktop/Notifications";
    const INTERFACE: &str = "org.freedesktop.Notifications";
    const SERVICE: &str = "org.freedesktop.Notifications";
    const SUMMARY: &str = "farm3d spike test — safe to ignore";
    const ICON: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/icons/128x128.png");

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
        fn close_notification(&self, id: u32) -> zbus::Result<()>;
        fn get_capabilities(&self) -> zbus::Result<Vec<String>>;
        fn get_server_information(&self) -> zbus::Result<(String, String, String, String)>;
    }

    fn log(msg: impl AsRef<str>) {
        static START: OnceLock<Instant> = OnceLock::new();
        let ms = START.get_or_init(Instant::now).elapsed().as_millis();
        println!("[{ms:>6}ms] {}", msg.as_ref());
    }

    /// Every `Focused(bool)` the window reported, with its log time.
    #[derive(Default)]
    struct FocusLog {
        events: Mutex<Vec<bool>>,
        focused: AtomicBool,
    }

    impl FocusLog {
        fn record(&self, focused: bool) {
            self.focused.store(focused, Ordering::SeqCst);
            self.events.lock().unwrap().push(focused);
            log(format!("  event WindowEvent::Focused({focused})"));
        }
        fn count(&self) -> usize {
            self.events.lock().unwrap().len()
        }
        fn since(&self, mark: usize) -> Vec<bool> {
            self.events.lock().unwrap()[mark..].to_vec()
        }
    }

    /// What the single long-lived listener tracks: farm3d's own ids, and
    /// an activation token per id until `ActionInvoked` consumes it.
    #[derive(Default)]
    struct Outstanding {
        ids: Mutex<HashSet<u32>>,
        tokens: Mutex<HashMap<u32, String>>,
        closed: Mutex<Vec<(u32, u32)>>,
    }

    fn hints(urgency: u8) -> HashMap<&'static str, Value<'static>> {
        HashMap::from([
            ("desktop-entry", Value::from("farm3d")),
            ("urgency", Value::U8(urgency)),
        ])
    }

    pub fn run() {
        log("notify_spike starting");
        let click_mode = std::env::args().any(|a| a == "--click");
        let focus_only = std::env::args().any(|a| a == "--focus-only");
        let focus = Arc::new(FocusLog::default());

        let mut context = tauri::generate_context!();
        // The spike opens its own small window, not the app's main one.
        context.config_mut().app.windows.clear();

        let focus_for_setup = focus.clone();
        tauri::Builder::default()
            .setup(move |app| {
                let window = WebviewWindowBuilder::new(
                    app,
                    "spike",
                    WebviewUrl::External("about:blank".parse().expect("static url")),
                )
                .title("farm3d notify spike (safe to ignore)")
                .inner_size(420.0, 200.0)
                .build()?;
                let focus_events = focus_for_setup.clone();
                window.on_window_event(move |event| {
                    if let WindowEvent::Focused(f) = event {
                        focus_events.record(*f);
                    }
                });
                let handle = app.handle().clone();
                let focus = focus_for_setup.clone();
                std::thread::spawn(move || {
                    let code = tauri::async_runtime::block_on(async {
                        let result = if click_mode {
                            click(&handle, &window, &focus).await
                        } else if focus_only {
                            focus_steps(&handle, &window, &focus).await;
                            Ok(())
                        } else {
                            automated(&handle, &window, &focus).await
                        };
                        match result {
                            Ok(()) => 0,
                            Err(e) => {
                                log(format!("FAILED: {e}"));
                                1
                            }
                        }
                    });
                    log("done; exiting");
                    handle.exit(code);
                });
                Ok(())
            })
            .run(context)
            .expect("tauri run");
    }

    struct Bus {
        conn: Connection,
        proxy: NotificationsProxy<'static>,
        outstanding: Arc<Outstanding>,
    }

    async fn connect_and_listen(
        on_action: Option<Arc<dyn Fn(u32, String, Option<String>) + Send + Sync>>,
    ) -> zbus::Result<Bus> {
        let conn = Connection::session().await?;
        log(format!(
            "session bus connected; our unique name {}",
            conn.unique_name().map(|n| n.as_str()).unwrap_or("?")
        ));
        let proxy = NotificationsProxy::new(&conn).await?;

        let (name, vendor, version, spec) = proxy.get_server_information().await?;
        log(format!(
            "GetServerInformation: name={name:?} vendor={vendor:?} version={version:?} spec_version={spec:?}"
        ));
        let caps = proxy.get_capabilities().await?;
        log(format!("GetCapabilities: {caps:?}"));
        for c in ["actions", "body-markup", "persistence", "icon-static"] {
            log(format!(
                "  advertises {c:?}: {}",
                caps.iter().any(|x| x == c)
            ));
        }

        let dbus = DBusProxy::new(&conn).await?;
        let owner = dbus.get_name_owner(BusName::try_from(SERVICE)?).await?;
        log(format!("current owner of {SERVICE}: {owner}"));

        // The one listener D6 describes: signals from the server's unique
        // name only, on its path and interface; ids we didn't send ignored.
        let rule = MatchRule::builder()
            .msg_type(MessageType::Signal)
            .sender(owner.as_str())?
            .path(PATH)?
            .interface(INTERFACE)?
            .build();
        let mut filtered = MessageStream::for_match_rule(rule, &conn, Some(64)).await?;

        // Diagnostic only: the same match without the sender, to show
        // what the sender filter keeps out.
        let raw_rule = MatchRule::builder()
            .msg_type(MessageType::Signal)
            .path(PATH)?
            .interface(INTERFACE)?
            .build();
        let mut raw = MessageStream::for_match_rule(raw_rule, &conn, Some(64)).await?;

        let outstanding = Arc::new(Outstanding::default());
        let tracked = outstanding.clone();
        tauri::async_runtime::spawn(async move {
            while let Some(Ok(msg)) = filtered.next().await {
                let header = msg.header();
                let member = header.member().map(|m| m.to_string()).unwrap_or_default();
                let sender = header.sender().map(|s| s.to_string()).unwrap_or_default();
                let dest = header
                    .destination()
                    .map(|d| d.to_string())
                    .unwrap_or_else(|| "(broadcast)".into());
                let (id, detail) = match member.as_str() {
                    "ActionInvoked" | "ActivationToken" | "NotificationReplied" => {
                        match msg.body().deserialize::<(u32, String)>() {
                            Ok((id, s)) => (id, s),
                            Err(e) => (0, format!("bad body: {e}")),
                        }
                    }
                    "NotificationClosed" => match msg.body().deserialize::<(u32, u32)>() {
                        Ok((id, reason)) => (id, format!("reason {reason}")),
                        Err(e) => (0, format!("bad body: {e}")),
                    },
                    _ => (0, String::new()),
                };
                let known = tracked.ids.lock().unwrap().contains(&id);
                if !known {
                    log(format!(
                        "[filtered] {member}({id}, {detail}) from {sender} to {dest}: IGNORED (id not ours)"
                    ));
                    continue;
                }
                log(format!(
                    "[filtered] {member}({id}, {detail}) from {sender} to {dest}: ours"
                ));
                match member.as_str() {
                    "ActivationToken" => {
                        tracked.tokens.lock().unwrap().insert(id, detail);
                    }
                    "ActionInvoked" => {
                        let token = tracked.tokens.lock().unwrap().remove(&id);
                        log(format!(
                            "  token present at ActionInvoked: {}",
                            token.is_some()
                        ));
                        if let Some(cb) = &on_action {
                            cb(id, detail, token);
                        }
                    }
                    "NotificationClosed" => {
                        let reason = detail.trim_start_matches("reason ").parse().unwrap_or(0);
                        tracked.ids.lock().unwrap().remove(&id);
                        tracked.tokens.lock().unwrap().remove(&id);
                        tracked.closed.lock().unwrap().push((id, reason));
                    }
                    _ => {}
                }
            }
            log("[filtered] stream ended");
        });
        tauri::async_runtime::spawn(async move {
            while let Some(Ok(msg)) = raw.next().await {
                let header = msg.header();
                log(format!(
                    "[raw]      {} from {}",
                    header.member().map(|m| m.to_string()).unwrap_or_default(),
                    header.sender().map(|s| s.to_string()).unwrap_or_default()
                ));
            }
        });

        Ok(Bus {
            conn,
            proxy,
            outstanding,
        })
    }

    async fn pause(ms: u64) {
        tokio::time::sleep(Duration::from_millis(ms)).await;
    }

    async fn automated(
        app: &AppHandle,
        window: &WebviewWindow,
        focus: &Arc<FocusLog>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        log(format!(
            "icon path exists: {}",
            std::path::Path::new(ICON).exists()
        ));
        let bus = connect_and_listen(None).await?;
        let actions = ["default", "Open"];

        // 1. Notify with D6's shape.
        let a = bus
            .proxy
            .notify(
                "farm3d",
                0,
                ICON,
                SUMMARY,
                "Notification 1 of 3. farm3d closes it itself in a few seconds.",
                &actions,
                hints(1),
                -1,
            )
            .await?;
        bus.outstanding.ids.lock().unwrap().insert(a);
        log(format!("Notify #1 returned id {a}"));
        pause(1500).await;

        // 2. replaces_id updates in place (the summary notification).
        let a2 = bus
            .proxy
            .notify(
                "farm3d",
                a,
                ICON,
                SUMMARY,
                "Notification 1 of 3, updated in place (replaces_id).",
                &actions,
                hints(1),
                -1,
            )
            .await?;
        log(format!(
            "Notify replaces_id={a} returned id {a2} (same id: {})",
            a == a2
        ));
        bus.outstanding.ids.lock().unwrap().insert(a2);
        pause(1500).await;

        // 3. Sender filtering: another connection forges NotificationClosed
        //    for our id. The raw stream sees it; the filtered one must not.
        let other = Connection::session().await?;
        log(format!(
            "second connection {} forges NotificationClosed({a}, 99)",
            other.unique_name().map(|n| n.as_str()).unwrap_or("?")
        ));
        other
            .emit_signal(
                None::<BusName<'_>>,
                PATH,
                INTERFACE,
                "NotificationClosed",
                &(a, 99u32),
            )
            .await?;
        pause(1000).await;
        log(format!(
            "id {a} still outstanding after the forgery: {}",
            bus.outstanding.ids.lock().unwrap().contains(&a)
        ));

        // 4. Id filtering: a real notification from the second connection.
        //    If the server broadcasts its NotificationClosed, the filtered
        //    stream receives it and must ignore it as not ours.
        let other_proxy = NotificationsProxy::new(&other).await?;
        let b = other_proxy
            .notify(
                "farm3d-spike-other-client",
                0,
                ICON,
                SUMMARY,
                "Notification 2 of 3, from a second D-Bus connection.",
                &[],
                HashMap::new(),
                -1,
            )
            .await?;
        log(format!("second connection Notify returned id {b}"));
        pause(1000).await;
        other_proxy.close_notification(b).await?;
        log(format!("second connection CloseNotification({b})"));
        pause(1000).await;

        // 5. CloseNotification on ours: NotificationClosed(a, 3) expected.
        bus.proxy.close_notification(a).await?;
        log(format!("CloseNotification({a})"));
        pause(1000).await;

        // 6. A timed notification: NotificationClosed(c, 1) if it expires.
        let c = bus
            .proxy
            .notify(
                "farm3d",
                0,
                ICON,
                SUMMARY,
                "Notification 3 of 3, expire_timeout 2000 ms.",
                &actions,
                hints(0),
                2000,
            )
            .await?;
        bus.outstanding.ids.lock().unwrap().insert(c);
        log(format!("Notify #3 (expire_timeout 2000) returned id {c}"));
        pause(3500).await;
        let expired = bus.outstanding.ids.lock().unwrap().contains(&c);
        log(format!("id {c} still outstanding after 3.5 s: {expired}"));
        bus.proxy.close_notification(c).await?;
        log(format!("CloseNotification({c}) (cleanup)"));
        pause(1000).await;
        log(format!(
            "NotificationClosed received for ours: {:?}",
            bus.outstanding.closed.lock().unwrap()
        ));
        drop(bus.conn);

        focus_steps(app, window, focus).await;
        Ok(())
    }

    async fn state(window: &WebviewWindow) -> String {
        format!(
            "is_focused={:?} is_minimized={:?} is_visible={:?}",
            window.is_focused().ok(),
            window.is_minimized().ok(),
            window.is_visible().ok()
        )
    }

    /// Run `f` on the main thread (one tick), then wait and report which
    /// `Focused` events arrived and what the getters say.
    async fn step<F>(app: &AppHandle, window: &WebviewWindow, focus: &FocusLog, label: &str, f: F)
    where
        F: FnOnce(&WebviewWindow) + Send + 'static,
    {
        let mark = focus.count();
        log(format!("STEP {label}"));
        let w = window.clone();
        app.run_on_main_thread(move || f(&w)).expect("main thread");
        pause(1500).await;
        log(format!(
            "  -> Focused events {:?}; {}",
            focus.since(mark),
            state(window).await
        ));
    }

    async fn focus_steps(app: &AppHandle, window: &WebviewWindow, focus: &Arc<FocusLog>) {
        log(format!(
            "focus steps begin; events so far {:?}; {}",
            focus.since(0),
            state(window).await
        ));
        step(app, window, focus, "set_focus", |w| {
            let _ = w.set_focus();
        })
        .await;
        step(app, window, focus, "minimize", |w| {
            let _ = w.minimize();
        })
        .await;
        step(app, window, focus, "unminimize (alone)", |w| {
            let _ = w.unminimize();
        })
        .await;
        step(
            app,
            window,
            focus,
            "set_focus (alone, after unminimize)",
            |w| {
                let _ = w.set_focus();
            },
        )
        .await;
        step(app, window, focus, "minimize", |w| {
            let _ = w.minimize();
        })
        .await;
        step(
            app,
            window,
            focus,
            "unminimize + show + set_focus in one tick",
            |w| {
                log(format!(
                    "  before: is_minimized={:?}",
                    w.is_minimized().ok()
                ));
                let _ = w.unminimize();
                log(format!(
                    "  after unminimize, same tick: is_minimized={:?}",
                    w.is_minimized().ok()
                ));
                let _ = w.show();
                let _ = w.set_focus();
            },
        )
        .await;
        step(app, window, focus, "set_focus (next tick)", |w| {
            let _ = w.set_focus();
        })
        .await;
        step(app, window, focus, "hide", |w| {
            let _ = w.hide();
        })
        .await;
        step(app, window, focus, "show (alone)", |w| {
            let _ = w.show();
        })
        .await;
        step(app, window, focus, "hide", |w| {
            let _ = w.hide();
        })
        .await;
        step(app, window, focus, "show + set_focus in one tick", |w| {
            let _ = w.show();
            let _ = w.set_focus();
        })
        .await;
        step(app, window, focus, "minimize", |w| {
            let _ = w.minimize();
        })
        .await;
        step(
            app,
            window,
            focus,
            "set_startup_id(invalid token) + unminimize + show + set_focus",
            |w| {
                if let Ok(gw) = w.gtk_window() {
                    gw.set_startup_id("farm3d-spike-invalid-token");
                }
                let _ = w.unminimize();
                let _ = w.show();
                let _ = w.set_focus();
            },
        )
        .await;
        step(
            app,
            window,
            focus,
            "hide + show + set_focus in one tick",
            |w| {
                let _ = w.hide();
                let _ = w.show();
                let _ = w.set_focus();
            },
        )
        .await;
        step(app, window, focus, "minimize", |w| {
            let _ = w.minimize();
        })
        .await;
        step(
            app,
            window,
            focus,
            "hide + show + set_focus in one tick",
            |w| {
                let _ = w.hide();
                let _ = w.show();
                let _ = w.set_focus();
            },
        )
        .await;
        step(app, window, focus, "minimize", |w| {
            let _ = w.minimize();
        })
        .await;
        step(
            app,
            window,
            focus,
            "request_user_attention(Informational)",
            |w| {
                let _ = w.request_user_attention(Some(UserAttentionType::Informational));
            },
        )
        .await;
        step(app, window, focus, "request_user_attention(None)", |w| {
            let _ = w.request_user_attention(None);
        })
        .await;
    }

    /// For Task 17: the owner clicks the notification and watches.
    async fn click(
        app: &AppHandle,
        window: &WebviewWindow,
        focus: &Arc<FocusLog>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let (done_tx, mut done_rx) = tokio::sync::mpsc::channel::<()>(1);
        let app_for_cb = app.clone();
        let window_for_cb = window.clone();
        let focus_for_cb = focus.clone();
        let on_action: Arc<dyn Fn(u32, String, Option<String>) + Send + Sync> =
            Arc::new(move |id, action, token| {
                log(format!(
                    "ActionInvoked({id}, {action:?}); raising the window"
                ));
                let w = window_for_cb.clone();
                let mark = focus_for_cb.count();
                let _ = app_for_cb.run_on_main_thread(move || {
                    if let Some(token) = &token {
                        match w.gtk_window() {
                            Ok(gw) => {
                                gw.set_startup_id(token);
                                log("  set_startup_id(token) applied");
                            }
                            Err(e) => log(format!("  gtk_window failed: {e}")),
                        }
                    }
                    let _ = w.unminimize();
                    let _ = w.show();
                    let _ = w.set_focus();
                    log("  unminimize + show + set_focus sent");
                });
                let w = window_for_cb.clone();
                let focus = focus_for_cb.clone();
                let done = done_tx.clone();
                tauri::async_runtime::spawn(async move {
                    pause(500).await;
                    let events = focus.since(mark);
                    log(format!("  Focused events within 500 ms: {events:?}"));
                    if !events.contains(&true) {
                        log("  no Focused(true): request_user_attention(Informational)");
                        let _ = w.request_user_attention(Some(UserAttentionType::Informational));
                    }
                    pause(2500).await;
                    log(format!(
                        "  3 s after the click: Focused events {:?}; {}",
                        focus.since(mark),
                        state(&w).await
                    ));
                    let _ = done.send(()).await;
                });
            });
        let bus = connect_and_listen(Some(on_action)).await?;

        log("minimizing the spike window; bring another app to the front if you like");
        let w = window.clone();
        app.run_on_main_thread(move || {
            let _ = w.minimize();
        })?;
        pause(2000).await;

        let id = bus
            .proxy
            .notify(
                "farm3d",
                0,
                ICON,
                "farm3d spike (Task 17): click me",
                "Click this notification. farm3d should come to the front.",
                &["default", "Open"],
                hints(1),
                -1,
            )
            .await?;
        bus.outstanding.ids.lock().unwrap().insert(id);
        log(format!(
            "Notify returned id {id}; waiting up to 120 s for a click"
        ));

        let clicked = tokio::time::timeout(Duration::from_secs(120), done_rx.recv())
            .await
            .is_ok();
        if !clicked {
            log("no click within 120 s");
        }
        let _ = bus.proxy.close_notification(id).await;
        log(format!("CloseNotification({id}) (cleanup)"));
        pause(500).await;
        Ok(())
    }
}
