# Desktop notifications over D-Bus

**Status:** Accepted, 2026-09-27, with the P8 spec. The click-activation
sequence is to be confirmed by the P8 notification spike (Task 2,
automatable parts) and the installed-bundle pass (Task 17, owner at the
desktop).

## Context

The umbrella spec requires desktop notifications, on by default for
fatal failures, operator confirmations, and Job completion, sent only
while farm3d is unfocused, each deep-linking to the exact Printer, Job,
Spool, or Incident. Clicking one must bring farm3d forward and open that
object. Linux x86_64 is the only supported platform (F0), and the owner's
session is KDE Plasma on Wayland.

Research on 2026-09-27 found:

- **`tauri-plugin-notification` 2.5.0** registers only `notify`,
  `request_permission`, and `is_permission_granted` on desktop. Its
  desktop implementation ignores actions, and `onAction` fires on mobile
  only (plugins-workspace#1903). It cannot tell farm3d that a
  notification was clicked, so it cannot deep-link. Desktop permission is
  always `Granted`.
- **`notify-rust` 4.18.1**, which the plugin uses, blocks one thread per
  notification to wait for an action, and ignores `ActivationToken`.
- **The freedesktop Desktop Notifications spec** has everything needed:
  the `default` action, `ActionInvoked`, `ActivationToken` (so a Wayland
  compositor lets the clicked app raise itself), and
  `NotificationClosed`, plus the `desktop-entry` hint for attribution.
  The owner's notification server (Plasma 6.7.5) reports spec version
  **1.2** from `GetServerInformation` and implements all of these,
  including the `ActivationToken` signal (P8 notification spike,
  [`2026-09-27-p8-notification-spike.md`](../superpowers/baselines/2026-09-27-p8-notification-spike.md)).
  farm3d doesn't gate on the reported version.
- **Focus.** Tauri's `is_focused()` is unreliable on Linux (tauri#11323);
  `WindowEvent::Focused` is the dependable signal. Tao's `set_focus` does
  nothing for a minimized window, and Wayland needs the activation token
  applied first (`gtk_window.set_startup_id(token)`, then present).
- `zbus` 5 is already in the lockfile, through `keyring`.

| Option | For | Against |
|---|---|---|
| A. `tauri-plugin-notification` | Official, cross-platform, no D-Bus code | No click on desktop, so no deep link; no activation token, so no raise on Wayland |
| B. `notify-rust` directly with its action handling | Small API | A blocked thread per notification; ignores `ActivationToken`; still needs our own focus and raise code |
| C. farm3d's own `org.freedesktop.Notifications` client over zbus, behind a `NotificationSink` trait | Reports clicks and activation tokens; one async connection for all notifications; full control of hints, urgency, and in-place replacement; testable with a recording sink | Linux-only code we maintain; other platforms need their own sinks later |

## Decision

**Option C.** farm3d sends notifications through its own D-Bus client,
and does not add `tauri-plugin-notification`.

- A `NotificationService` in Rust decides whether to notify with a pure
  `decide` function (the Event was newly inserted live, its class is
  enabled, the Printer isn't muted, the window is unfocused, and the rate
  limits allow it). The frontend never decides.
- On Linux, `DbusNotificationSink` holds one long-lived zbus session
  connection. It calls `Notify` with the `default` action, the
  `desktop-entry=farm3d` hint, an urgency from the severity, and the
  `farm3d` icon (by name when the icon theme has it, else by absolute
  path). It listens for `ActionInvoked`, `ActivationToken`, and
  `NotificationClosed`, matched on the notification server's unique bus
  name and on farm3d's own notification ids.
- On a click: apply the activation token if one arrived, unminimize,
  show, and focus the window on the main thread (falling back to
  `request_user_attention`), emit `farm3d-navigate-v1` with the target,
  and mark the Event read.
- Focus comes from `WindowEvent::Focused` only, and starts as focused, so
  nothing notifies before the first focus-out.
- Other platforms get a `NullNotificationSink` that reports
  `unsupported`. The in-app Attention center works everywhere and always
  keeps every Event.
- The freedesktop protocol has no permission model, so farm3d asks for
  none. `notification_status` reports whether a notification server is
  reachable, and Do Not Disturb stays the daemon's business.

## Consequences

- Clicking a notification opens the exact source, which is the reason
  for the choice.
- farm3d owns a small amount of D-Bus code, with a direct `zbus`
  dependency for Linux targets pinned to the lockfile's major version.
- Windows and macOS have no desktop notifications in P8, and are
  recorded as unverified. Adding them means a new sink per platform, not
  a change to the policy or the Attention model.
- Whether the raise works depends on the daemon and compositor sending an
  `ActivationToken`. Where one doesn't, the window may only flash in the
  taskbar, and navigation still happens. The spike and the installed-bundle
  pass record the observed behavior.
- A session without a notification server (or without a session bus)
  degrades to `unavailable`, never to a crash.
