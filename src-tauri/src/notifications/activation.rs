//! P8 D6 "Click activation": what a click on one of farm3d's
//! notifications does — raise the window, emit [`NAVIGATE_EVENT`], and,
//! when the notification names one Event, mark it read.
//!
//! **The raise** (controller ruling, from the Task 2 spike on KWin
//! Wayland): one main-thread tick applies the activation token, if one
//! arrived, to the GTK window (`set_startup_id`), then `show` and
//! `set_focus`. If no `Focused(true)` follows within the wait (500 ms),
//! `hide`, `show`, and `set_focus` in one tick (re-mapping brings back a
//! minimized window without a token), and another wait; only then
//! `request_user_attention(Informational)`, which may be a no-op on
//! Wayland. `unminimize` is not used: it does nothing on Wayland. The
//! raise never blocks the navigation: step 4 and 5 run at once, and the
//! Attention center keeps the Event whatever the raise does. Task 17
//! (the owner at the desktop) confirms the sequence with a real click.

use std::sync::Arc;
use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager};

use super::focus::Focus;
use super::{NavigateRequest, NAVIGATE_EVENT};

/// The main window's label (the one window `tauri.conf.json` declares).
pub const MAIN_WINDOW: &str = "main";

/// One raise step, as [`WindowControl`] performs it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum RaiseStep {
    /// Apply `token` if any, then `show` and `set_focus`, in one tick.
    Present { token: Option<String> },
    /// `hide`, `show`, and `set_focus` in one tick.
    Remap,
    /// `request_user_attention(Informational)`.
    RequestAttention,
}

/// The window operations a raise needs. Each call is one main-thread
/// tick. Production drives the main window ([`MainWindowControl`]); tests
/// record the steps.
pub trait WindowControl: Send + Sync {
    fn present(&self, token: Option<String>);
    fn remap(&self);
    fn request_attention(&self);
}

/// Drives farm3d's main window on the main thread.
pub struct MainWindowControl<R: tauri::Runtime> {
    app: AppHandle<R>,
}

impl<R: tauri::Runtime> MainWindowControl<R> {
    pub fn new(app: AppHandle<R>) -> Self {
        Self { app }
    }

    fn on_main_window(&self, act: impl FnOnce(&tauri::WebviewWindow<R>) + Send + 'static) {
        let app = self.app.clone();
        let _ = self.app.run_on_main_thread(move || {
            if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
                act(&window);
            }
        });
    }
}

impl<R: tauri::Runtime> WindowControl for MainWindowControl<R> {
    fn present(&self, token: Option<String>) {
        self.on_main_window(move |window| {
            #[cfg(target_os = "linux")]
            if let Some(token) = &token {
                use gtk::prelude::GtkWindowExt;
                if let Ok(gtk_window) = window.gtk_window() {
                    gtk_window.set_startup_id(token);
                }
            }
            #[cfg(not(target_os = "linux"))]
            let _ = &token;
            let _ = window.show();
            let _ = window.set_focus();
        });
    }

    fn remap(&self) {
        self.on_main_window(|window| {
            let _ = window.hide();
            let _ = window.show();
            let _ = window.set_focus();
        });
    }

    fn request_attention(&self) {
        self.on_main_window(|window| {
            let _ = window.request_user_attention(Some(tauri::UserAttentionType::Informational));
        });
    }
}

/// Step 3: presents the window now, then runs the fallbacks on their own
/// task while no `Focused(true)` arrives. Never waits.
pub fn raise(control: Arc<dyn WindowControl>, focus: Focus, token: Option<String>, wait: Duration) {
    let mark = focus.gained();
    control.present(token);
    tauri::async_runtime::spawn(async move {
        if focus.regained_since(mark, wait).await {
            return;
        }
        control.remap();
        if focus.regained_since(mark, wait).await {
            return;
        }
        control.request_attention();
    });
}

/// Step 4: the unsequenced `farm3d-navigate-v1` event. Its payload holds a
/// navigation target (ids only) and nothing else.
pub fn navigate<R: tauri::Runtime>(app: &AppHandle<R>, request: &NavigateRequest) {
    let _ = app.emit(NAVIGATE_EVENT, request);
}
