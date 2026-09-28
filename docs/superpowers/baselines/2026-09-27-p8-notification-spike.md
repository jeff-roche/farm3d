# P8 Notification and Focus Spike (Linux)

P8 Task 2, for spec D6 and ADR-0015. It checks the D-Bus notification
design on the owner's KDE Plasma Wayland session before Task 9 builds it.
Nobody was at the desktop, so the spike ran only what a program can check
by itself. Everything that needs a click or a pair of eyes is listed for
Task 17 at the end. No input automation was used.

## Decision

| Question | Result | Evidence |
|---|---|---|
| Is a notification server reachable, and what does it offer? | **Yes.** Plasma 6.7.5, spec 1.2, with `actions`, `body-markup`, `persistence`, `icon-static` | Run 1, `busctl` |
| Does D6's `Notify` call work as specified? | **Yes.** It returns an id | Run 1 |
| Does one long-lived zbus listener get `NotificationClosed` for farm3d's id? | **Yes**, reason 3 after `CloseNotification` | Run 1 |
| Does the sender filter keep out forged signals? | **Yes** | Run 1, forged signal |
| Is the id filter needed? | **Yes.** Plasma broadcasts its signals, so other clients' closes arrive too | Run 1, second client |
| Does `replaces_id` update in place? | **Yes.** Same id, no `NotificationClosed` | Run 1 |
| Can the program un-minimize its window without a token? | **Not with `unminimize` or `set_focus`.** A `hide` then `show` does it | Runs 1 and 2 |
| Is `set_focus` dropped right after `unminimize` in the same tick? | **Not on Wayland**, because `is_minimized()` is always `false` there. By tao's source, it would be on X11 | Runs 1 and 2, tao source |
| Does KWin send `ActivationToken` before `ActionInvoked`, and does the token raise the window? | **Unknown.** Deferred to Task 17 | Needs a click |
| Is the notification attributed to farm3d with its icon? | **Unknown.** Deferred to Task 17 | Needs a person to look |
| GNOME session | **Unavailable.** None on this host | — |

## Host and versions

| Item | Value |
|---|---|
| Session | KDE Plasma on Wayland (`XDG_SESSION_TYPE=wayland`, `XDG_CURRENT_DESKTOP=KDE`); session bus present; Xwayland `DISPLAY` also set |
| Notification server | `plasmashell` owns `org.freedesktop.Notifications`. `GetServerInformation` = `("Plasma", "KDE", "6.7.5", "1.2")` |
| Compositor | `kwin_wayland --version` = kwin 6.7.5; `plasmashell --version` = plasmashell 6.7.5 |
| KWin focus-stealing prevention | Not set in `kwinrc`, so the default (Low) |
| Toolkit | GTK 3.24.52, WebKitGTK 4.1 2.52.6 |
| Kernel | Linux 7.2.8-1-cachyos x86_64 |
| Toolchain | rustc 1.98.0 (88d9e12ae 2026-08-18), cargo 1.98.0 |
| Crates | `tauri` 2.11.5, `tauri-runtime-wry` 2.11.4, `tao` 0.35.3, `wry` 0.55.1, `zbus` 5.19.0 (default features: `async-io`, `blocking-api`), `gtk` 0.18.2 |

`GetCapabilities` returned, in order: `body`, `body-hyperlinks`,
`body-markup`, `body-images`, `icon-static`, `actions`, `persistence`,
`inline-reply`, `sound`, `x-kde-urls`, `x-kde-origin-name`,
`x-kde-display-appname`, `inhibitions`.

Introspecting `/org/freedesktop/Notifications` shows the signals
`ActionInvoked (us)`, `ActivationToken (us)`, `NotificationClosed (uu)`,
and `NotificationReplied (us)`, and the methods `Notify`,
`CloseNotification`, `GetCapabilities`, `GetServerInformation`, `Inhibit`,
and `UnInhibit`. So Plasma implements `ActivationToken`, even though it
reports spec version 1.2. ADR-0015 cites the notification spec as 1.3;
the server says 1.2.

## What ran

`src-tauri/examples/notify_spike.rs` is a Tauri app with one small window
and a zbus client. It has three modes:

```sh
source "$HOME/.cargo/env"
cargo run --manifest-path src-tauri/Cargo.toml --example notify_spike                 # run 1
cargo run --manifest-path src-tauri/Cargo.toml --example notify_spike -- --focus-only # run 2
cargo run --manifest-path src-tauri/Cargo.toml --example notify_spike -- --click      # Task 17 only
```

The example opens one session connection and subscribes two match rules:

- **filtered**: signals from the server's unique name, on its path and
  interface. This is D6's single listener. It ignores ids farm3d did not
  send.
- **raw**: the same rule without the sender. It is diagnostic only, and
  shows what the sender filter keeps out.

Run 1 sent three notifications and one in-place replacement. Each used the
summary "farm3d spike test — safe to ignore", and the spike closed each one
with `CloseNotification`. Run 2 sent none.

### Run 1: D-Bus

The unique bus names below are the session's own and change on every
connection.

```text
[   367ms] GetServerInformation: name="Plasma" vendor="KDE" version="6.7.5" spec_version="1.2"
[   368ms] current owner of org.freedesktop.Notifications: :1.25
[   380ms] Notify #1 returned id 103
[  1885ms] Notify replaces_id=103 returned id 103 (same id: true)
[  3388ms] second connection :1.981 forges NotificationClosed(103, 99)
[  3388ms] [raw]      NotificationClosed from :1.981
[  4389ms] id 103 still outstanding after the forgery: true
[  4399ms] second connection Notify returned id 104
[  5401ms] [filtered] NotificationClosed(104, reason 3) from :1.25 to (broadcast): IGNORED (id not ours)
[  5401ms] second connection CloseNotification(104)
[  6403ms] [filtered] NotificationClosed(103, reason 3) from :1.25 to (broadcast): ours
[  6403ms] CloseNotification(103)
[  7417ms] Notify #3 (expire_timeout 2000) returned id 105
[ 10919ms] id 105 still outstanding after 3.5 s: true
[ 10920ms] [filtered] NotificationClosed(105, reason 3) from :1.25 to (broadcast): ours
[ 11922ms] NotificationClosed received for ours: [(103, 3), (105, 3)]
```

`Notify #1` used D6's call:

- `app_name` "farm3d" and an absolute icon path (`src-tauri/icons/128x128.png`);
- actions `["default", "Open"]`;
- hints `desktop-entry` = "farm3d" and `urgency` = byte 1;
- `expire_timeout` −1.

Findings:

1. **Sender filter works.** The forged `NotificationClosed(103, 99)` from
   a second connection reached the raw rule only. Id 103 stayed
   outstanding.
2. **Plasma broadcasts its signals.** They have no destination. The
   second client's `NotificationClosed(104)` reached farm3d's filtered
   listener, and the id check dropped it. D6's "ids not in `outstanding`
   are ignored" rule is needed, not just defensive.
3. **`replaces_id` updates in place.** It returned the same id 103, and
   no `NotificationClosed` came for the replaced content. D6's burst
   summary can rely on this.
4. **`CloseNotification` is reported.** It yields `NotificationClosed(id,
   3)` on the long-lived listener.
5. **An expired timeout was not reported.** A notification with
   `expire_timeout` 2000 was still outstanding 3.5 s later. With
   `persistence`, Plasma probably moves an expired popup into its history
   instead of closing it; the spike did not look into why. The
   consequence: an id the operator never touches can stay outstanding,
   so D6's bound of 256 on `outstanding` is needed.

### Runs 1 and 2: focus

Each step ran its window calls in one `run_on_main_thread` closure, then
waited 1.5 s. It logged the `WindowEvent::Focused` values that arrived,
and the getters `is_focused` / `is_minimized` / `is_visible` (shown as
`f`, `m`, `v`).

| Step | Run 1 events | Run 2 events | Getters after (both runs) |
|---|---|---|---|
| window created | `[true]` | `[]` (opened unfocused) | f = run's event, m false, v true |
| `set_focus` | `[]` (already focused) | `[true]` | f true, m false, v true |
| `minimize` | `[false]` | `[false]` | f false, **m false**, v true |
| `unminimize` alone | `[]` | `[]` | f false, m false, v true |
| `set_focus` alone | `[]` | `[]` | f false, m false, v true |
| `minimize` (already) | `[]` | `[]` | f false, m false, v true |
| `unminimize` + `show` + `set_focus`, one tick | `[]` | `[]` | f false, m false, v true |
| `set_focus`, next tick | `[]` | `[]` | f false, m false, v true |
| `hide` (while minimized) | `[]` | `[]` | f false, m false, v false |
| `show` alone | `[true]` | `[true]` | f true, m false, v true |
| `hide` | `[false]` | `[false]` | f false, m false, v false |
| `show` + `set_focus`, one tick | `[true]` | `[true]` | f true, m false, v true |
| `minimize` | `[false]` | `[false]` | f false, m false, v true |
| `set_startup_id("…invalid…")` + `unminimize` + `show` + `set_focus` | `[]` | `[]` | f false, m false, v true |
| `hide` + `show` + `set_focus`, one tick (from minimized) | not run | `[true]` twice, 3–4 ms after the call | f true, m false, v true |
| `request_user_attention(Informational)` | `[]` | `[]` | f false, m false, v true |
| `request_user_attention(None)` | `[]` | `[]` | f false, m false, v true |

Findings:

1. **`Focused(false)` and `Focused(true)` fire when expected.**
   `Focused(false)` fires on `minimize` and `hide`. `Focused(true)` fires
   on `show` and on `set_focus` of a visible window. `is_focused()` agreed
   with the events at every step. tauri#11323 did not reproduce here, but
   D6 keeps the event as the source.
2. **`is_minimized()` is always `false` on Wayland**, even right after
   `minimize`. GTK gets no minimized state back from xdg-shell. tao's
   `set_focus` returns early only when its minimized flag is set (or the
   window isn't visible), and that flag only changes on a GTK
   window-state event. So on Wayland, `set_focus` right after `unminimize`
   in the same tick is **not** dropped by that guard. It just does
   nothing for a minimized window. The same-tick drop is an X11 concern,
   known from tao's source and not observed here.
3. **Nothing brings back a minimized window without a real token**:
   neither `unminimize` (GTK deiconify, which has no Wayland request),
   nor `set_focus` (GTK `present`), nor both with `show`, nor the same
   after an invalid `set_startup_id`. There was no `Focused(true)`.
4. **A hide/show cycle does bring it back.** `hide` then `show` restored
   a minimized window and gave it focus in both runs, and `hide` + `show`
   + `set_focus` in one tick did the same twice in run 2. The window is
   unmapped and mapped again, and KWin (focus-stealing prevention at its
   default) activates the newly mapped window. This is the fallback D6
   now names, pending the controller's approval.
5. **`set_focus` without a token activated a visible, unfocused window**
   (run 2: the window opened without focus, and the first `set_focus`
   gave `Focused(true)`). Which window was active before is unknown.
   Task 17 checks this against an app the owner is actively using.
6. **The window can open unfocused, with no `Focused(false)`.** D6 starts
   `Focus` as `true`, so in that case no notification goes out until the
   operator focuses the window and leaves it once. This errs on the quiet
   side. Seeding from `is_focused()` once the window is shown would close
   the gap. The controller decides before Task 9.
7. **`request_user_attention` produced no event**, as expected. tao maps
   it to `gtk_window_set_urgency_hint`. The only backend-specific urgency
   symbol in the host's `libgdk-3` is the X11 one, so the call may do
   nothing on Wayland. Whether the taskbar entry reacts is a Task 17 check.

## What works, and what falls back

| Behavior | Works here? | Fallback |
|---|---|---|
| Server discovery and `notification_status` fields | Yes | `unavailable` when there is no bus or server (D6) |
| `Notify` with actions, `desktop-entry`, urgency, icon path | Yes (call level) | — |
| Knowing when farm3d's notifications close | Yes for `CloseNotification`; not for an expired timeout | The 256 bound on `outstanding` |
| Ignoring other clients' and forged signals | Yes (sender and id filters) | — |
| Burst summary replaced in place | Yes | — |
| Raising a visible, unfocused window | Yes, with `set_focus` alone | — |
| Raising a minimized window after a click | Unknown with a token (Task 17). No with `unminimize`/`set_focus` alone | `hide` + `show` + `set_focus` in one tick, then `request_user_attention(Informational)`. Navigation and the Attention center work whatever the raise does |
| Focus tracking | Yes for minimize, hide, show, and `set_focus`. Alt-tab and file dialogs: Task 17 | Start `true`, so the window stays quiet until the first focus change |

## Task 17 checklist (owner at the desktop)

Run these on KDE Plasma Wayland: first under `just dev`, then from the
installed deb. `notify_spike -- --click` covers the first five. It
minimizes its window, sends one notification, waits up to two minutes for
a click, runs D6's step 3, and logs whether a token arrived and which
`Focused` events followed.

1. Clicking the notification body sends `ActionInvoked(id, "default")`.
   Clicking the "Open" button, if Plasma shows one, sends `"Open"`.
2. KWin/Plasma sends `ActivationToken(id, token)`, and it arrives
   **before** `ActionInvoked`. The spike logs "token present at
   ActionInvoked: true/false".
3. `set_startup_id(token)` + `unminimize` + `show` + `set_focus` raises
   the **minimized** window, and gives `Focused(true)` within 500 ms.
4. The same raises the window when it is **behind another app** that the
   owner is actively using (not minimized).
5. When 3 or 4 fails: the `hide` + `show` + `set_focus` fallback raises
   it. The re-mapped window keeps its position, size, and page contents
   (check in the real app, not the blank spike window). Record whether
   `request_user_attention` visibly marks the taskbar entry.
6. `Focused(false)` arrives on alt-tab away from farm3d, and around a
   GTK file dialog (farm3d's open-file picker). Record which way the
   dialog goes: does opening it send `Focused(false)`, and does closing
   it send `Focused(true)`?
7. The popup and the history entry show "farm3d" and farm3d's icon.
   Under `just dev` (no installed `farm3d.desktop`, icon by absolute
   path) and from the deb (desktop entry and hicolor icon installed).
   If the attribution is wrong under `just dev`, record whether it
   matters, and whether the KDE-specific `x-kde-display-appname` hint
   (advertised here) would fix it.
8. A notification that expires or is dismissed by hand: which
   `NotificationClosed` reason Plasma sends, if any, and when.
9. A GNOME session, if one becomes available: repeat 1–7. It was
   unavailable for Task 2.
