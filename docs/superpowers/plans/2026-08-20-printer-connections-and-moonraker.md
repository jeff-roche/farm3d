# Printer Connections + Moonraker Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make one Connection kind real end to end — configure a Moonraker printer, store its API key outside `printers.json`, discover it over mDNS, test the connection, and stream live temperatures and job state onto the dashboard.

**Architecture:** A `PrinterConnection` trait with one implementation (Moonraker over WebSocket JSON-RPC). A supervisor task per connected Printer owns reconnect-with-backoff and pushes `PrinterStatus` to the frontend as Tauri events. Credentials go to the OS keychain with a specified `credentials.json` fallback; `printers.json` stores only a reference. All protocol parsing lives in a pure, I/O-free module so the risky logic is unit-testable without a printer.

**Tech Stack:** Rust (Tauri 2, tokio, tokio-tungstenite, keyring, mdns-sd), TypeScript (SolidJS, Kobalte via the repo's design system), Vitest + `cargo test`.

**Spec:** `docs/superpowers/specs/2026-08-20-printer-connections-design.md`

## Global Constraints

Every task's requirements implicitly include this section.

- **A secret must never be written into `printers.json`, under any fallback tier.** `printers.json` stores only a `credentialRef` string. This is the property phase 1 deliberately designed the file to have — it stays safe to hand-edit, copy between machines, and paste into a bug report.
- **Never hardcode colors, font sizes, or radii in component CSS.** Reference `--f3d-color-*` / `--f3d-type-*` / `--f3d-radius-*` (see `DESIGN.md`). Hardcoding breaks theme switching.
- **CSS Modules only** (`Component.module.css` beside `Component.tsx`), never global CSS.
- **Prefer a design-system component over a raw Kobalte primitive or a hand-rolled control.** Everything this phase needs already exists: `TextField`, `Select`, `Button`, `Field`, `Chip`, `Panel`, `Tabs`. No new design-system component is required by this plan.
- **This is an editor/tool aesthetic** (Blender/Godot/Unity-inspired) — dense, flat, small radii, no elevation/ripple. Do not reintroduce Material Design patterns.
- `cargo`/`rustc` are **not on `PATH` in non-interactive shells**. Prefix every Rust command with `source "$HOME/.cargo/env" && `.
- **Verification gate:** `just build`, `just test`, and `just test-rust` must all pass before any task is considered done.
- **Test idiom (frontend):** Kobalte's `Select` and `DropdownMenu` triggers open on `pointerdown`, not `click`. Use `fireEvent.pointerDown` to open and `fireEvent.click` to pick a `Select` item. Use `vi.hoisted()` for mock fns referenced inside `vi.mock()` factories.
- **Test idiom (Rust):** follow `settings.rs` — path-taking pure functions plus an atomic-counter temp dir. The repo has **no dev-dependencies** and adds none here; no `tempfile`, no `tokio-test`.
- **`ConnectionState` is farm3d's vocabulary, not Moonraker's.** Keep it to `connecting | online | offline | error` so phase 3's adapters map onto it rather than widening it.
- **Every `PrinterStatus` field except `connectionState` and `updatedAt` is optional.** A partial update is the normal case, and a printer can be online with no job loaded.

---

## File Structure

**Create (Rust):**

| File | Responsibility |
|---|---|
| `src-tauri/src/connections/mod.rs` | Shared types + the `PrinterConnection` trait. No I/O. |
| `src-tauri/src/connections/credentials.rs` | Two-tier secret storage; knows nothing about protocols. |
| `src-tauri/src/connections/moonraker/protocol.rs` | **Pure** JSON-RPC framing + status merging. No sockets. |
| `src-tauri/src/connections/moonraker/mod.rs` | The socket-facing adapter. Thin: framing lives next door. |
| `src-tauri/src/connections/discovery.rs` | mDNS browse → `Vec<DiscoveredPrinter>`. |
| `src-tauri/src/connections/supervisor.rs` | Task-per-printer lifecycle, backoff, status fan-out. |
| `src-tauri/src/connections/commands.rs` | `#[tauri::command]` surface. Thin wrappers only. |

**Create (frontend):**

| File | Responsibility |
|---|---|
| `src/screens/PrinterConnectionPanel.tsx` (+ `.module.css`, `.test.tsx`) | The Connection tab. |

**Modify:** `src-tauri/Cargo.toml`, `src-tauri/src/lib.rs`, `src-tauri/src/printers.rs`, `src-tauri/src/catalog/resolve.rs`, `src/printers/types.ts`, `src/printers/printer-store.ts` (+ test), `src/screens/PrinterDashboard.tsx` (+ `.module.css`, test), `src/App.tsx`, `docs/superpowers/specs/2026-08-20-printer-connections-design.md` (only if implementation contradicts it).

**Why `protocol.rs` is split from `moonraker/mod.rs`:** the partial-update merge is the highest-risk logic in the phase and the part most worth testing exhaustively. Keeping it in a module with no sockets means its tests are plain `#[test]` functions over JSON literals — no async runtime, no network, no fixtures.

---

## Ordering and risk

Tasks 1–3 are pure Rust with no I/O and no Tauri — highest test value, lowest integration friction, so they come first. Task 4 is the first code that opens a socket. Tasks 8–10 are the frontend, which can be developed against the `just web` fallback without a printer.

**The single highest-risk detail in this phase** is Moonraker's partial `notify_status_update`. Task 3 exists to get it right in isolation before any socket code depends on it.

---

### Task 1: Connection types and the typed `connection` slot

Adds every new dependency and replaces phase 1's deliberately-opaque `connection: Option<serde_json::Value>` with a real type. Nothing here talks to a network.

**Files:**
- Modify: `src-tauri/Cargo.toml`
- Create: `src-tauri/src/connections/mod.rs`
- Modify: `src-tauri/src/lib.rs` (add `pub mod connections;`)
- Modify: `src-tauri/src/printers.rs:34` (the `connection` field)
- Modify: `src-tauri/src/catalog/resolve.rs:39` and `:208` (the `connection` field and its passthrough)

**Interfaces:**
- Produces: `ConnectionConfig`, `ConnectionState`, `PrinterStatus`, `ProbeResult`, `ReportedCapabilities`, `ConnectionError`, `PrinterConnection`, `MOONRAKER_KIND`, `DEFAULT_MOONRAKER_PORT`. Every later task consumes these.

- [ ] **Step 1: Add the dependencies**

These exact versions were compile-verified together while writing this plan. `keyring` needs no feature flags — its default `v1` feature selects Keychain Services / Credential Manager / Secret Service per platform. `tokio` gets only the features Tauri's runtime does not already provide us; we spawn through `tauri::async_runtime::spawn`, never `#[tokio::main]`.

In `src-tauri/Cargo.toml`, under `[dependencies]`:

```toml
tokio = { version = "1.53", features = ["sync", "time", "rt"] }
tokio-tungstenite = "0.30"
async-trait = "0.1"
futures-util = "0.3"
keyring = "4.1"
mdns-sd = "0.21"
```

- [ ] **Step 2: Verify the dependency tree resolves**

Run: `source "$HOME/.cargo/env" && cargo fetch --manifest-path src-tauri/Cargo.toml`
Expected: completes without a version-resolution error.

This is the moment `src-tauri` stops being a thin file-IO shim and becomes a long-running process with background state. That is intended, not incidental.

- [ ] **Step 3: Write the failing test for connection-config serde**

Create `src-tauri/src/connections/mod.rs` containing **only** this test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_config_uses_camel_case_and_omits_absent_credential() {
        let config = ConnectionConfig {
            kind: MOONRAKER_KIND.to_string(),
            host: "voron.local".to_string(),
            port: DEFAULT_MOONRAKER_PORT,
            use_tls: false,
            credential_ref: None,
        };
        let json = serde_json::to_string(&config).unwrap();
        assert_eq!(
            json,
            r#"{"kind":"moonraker","host":"voron.local","port":7125,"useTls":false}"#
        );
    }

    #[test]
    fn connection_config_round_trips_with_a_credential_ref() {
        let config = ConnectionConfig {
            kind: MOONRAKER_KIND.to_string(),
            host: "10.0.0.5".to_string(),
            port: 7125,
            use_tls: true,
            credential_ref: Some("farm3d/printer/prn-1/apikey".to_string()),
        };
        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains(r#""credentialRef":"farm3d/printer/prn-1/apikey""#));
        assert_eq!(serde_json::from_str::<ConnectionConfig>(&json).unwrap(), config);
    }

    #[test]
    fn connection_state_serializes_camel_case() {
        assert_eq!(
            serde_json::to_string(&ConnectionState::Connecting).unwrap(),
            r#""connecting""#
        );
    }

    #[test]
    fn printer_status_omits_every_absent_field() {
        let status = PrinterStatus::new(ConnectionState::Offline);
        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains(r#""connectionState":"offline""#));
        // A status with no readings must not claim zero temperatures.
        assert!(!json.contains("nozzleTempC"));
        assert!(!json.contains("progress"));
    }
}
```

- [ ] **Step 4: Run it to confirm it fails**

Run: `source "$HOME/.cargo/env" && cargo test --manifest-path src-tauri/Cargo.toml connections::`
Expected: FAIL — `cannot find type ConnectionConfig in this scope` (plus a `file not found for module connections` error until Step 6 wires `lib.rs`).

- [ ] **Step 5: Write the types and the trait**

Prepend to `src-tauri/src/connections/mod.rs`, above the test module:

```rust
//! Connection configuration, live status, and the adapter trait every
//! protocol implements.
//!
//! Phase 2 ships exactly one adapter (Moonraker). Phase 3 adds OctoPrint and
//! ElegooLink against this same trait — and is expected to reshape it, which
//! is why the trait is deliberately two methods wide and why `ConnectionState`
//! is farm3d's own small vocabulary rather than any one protocol's.

pub mod credentials;

use serde::{Deserialize, Serialize};

pub const MOONRAKER_KIND: &str = "moonraker";
pub const DEFAULT_MOONRAKER_PORT: u16 = 7125;

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionConfig {
    /// `"moonraker"` today; phase 3 adds `"octoprint"` and `"elegoolink"`.
    /// A free string rather than an enum so an unknown kind written by a
    /// newer farm3d round-trips through an older one instead of failing the
    /// whole `printers.json` load.
    pub kind: String,
    pub host: String,
    pub port: u16,
    #[serde(default)]
    pub use_tls: bool,
    /// A key INTO the credential store — never the secret itself. See the
    /// module docs on `credentials`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_ref: Option<String>,
}

/// farm3d's connection vocabulary, deliberately orthogonal to the print-job
/// state the card already shows. A live socket to a shut-down Klipper is
/// `Offline`, not `Online`.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum ConnectionState {
    Connecting,
    Online,
    Offline,
    Error,
}

/// Everything the dashboard renders live.
///
/// Every reading is `Option` for two independent reasons: Moonraker sends
/// PARTIAL updates (see `moonraker::protocol`), and a printer can be online
/// with no job loaded and no heaters configured. A `None` means "not
/// reported", which must render as an em dash — never as `0`.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PrinterStatus {
    pub connection_state: ConnectionState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job_state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job_name: Option<String>,
    /// `0.0..=1.0`, from `display_status.progress`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nozzle_temp_c: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nozzle_target_c: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bed_temp_c: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bed_target_c: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub print_duration_s: Option<f64>,
    /// farm3d's own clock, deliberately NOT Moonraker's `eventtime` — that
    /// value is a Klipper-uptime float, meaningless to a user and
    /// incomparable across printers.
    pub updated_at: String,
}

impl PrinterStatus {
    pub fn new(connection_state: ConnectionState) -> Self {
        Self {
            connection_state,
            error: None,
            job_state: None,
            job_name: None,
            progress: None,
            nozzle_temp_c: None,
            nozzle_target_c: None,
            bed_temp_c: None,
            bed_target_c: None,
            print_duration_s: None,
            updated_at: crate::printers::now_rfc3339(),
        }
    }

    pub fn errored(message: impl Into<String>) -> Self {
        let mut status = Self::new(ConnectionState::Error);
        status.error = Some(message.into());
        status
    }
}

/// What "Test connection" renders. Its job is to let a user confirm they
/// reached the machine they meant to reach.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ProbeResult {
    pub kind: String,
    /// The host software, e.g. Moonraker's own version.
    pub host_software: String,
    /// The firmware behind it, e.g. Klipper's `software_version`.
    pub firmware: String,
    pub reported_name: String,
    pub state: String,
    pub state_message: String,
    pub reported: ReportedCapabilities,
}

/// The build volume the HOST reports, for cross-checking against the catalog
/// Profile. All `Option` — an unhomed or shut-down Klipper reports no axis
/// limits at all, which is a normal state and not an error.
///
/// The comparison itself happens in the frontend, which already holds the
/// `ResolvedPrinter` carrying the catalog's numbers; doing it here would mean
/// a second catalog lookup for no gain.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct ReportedCapabilities {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bed_width_mm: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bed_depth_mm: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub printable_height_mm: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConnectionError {
    /// Could not open a socket at all — wrong host, wrong port, host down.
    Unreachable(String),
    /// Reached the host; it rejected our credentials.
    Auth(String),
    /// Reached the host; it spoke something we could not parse.
    Protocol(String),
    Timeout,
}

impl std::fmt::Display for ConnectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConnectionError::Unreachable(m) => write!(f, "Could not reach the printer: {m}"),
            ConnectionError::Auth(m) => write!(f, "The printer rejected the credentials: {m}"),
            ConnectionError::Protocol(m) => write!(f, "Unexpected response from the printer: {m}"),
            ConnectionError::Timeout => write!(f, "The printer did not respond in time"),
        }
    }
}

impl std::error::Error for ConnectionError {}

/// One protocol adapter.
///
/// `subscribe` owns its transport and pushes into a channel rather than
/// exposing a `poll()` the supervisor ticks: Moonraker and ElegooLink push
/// over WebSocket while OctoPrint will drive its own interval internally in
/// phase 3, and neither shape should leak into the supervisor.
///
/// An adapter never implements retry. The supervisor owns
/// reconnect-with-backoff, so `subscribe` returning `Ok(())` means "the
/// stream ended cleanly" and returning `Err` means "it failed"; both are the
/// supervisor's cue to reconnect.
#[async_trait::async_trait]
pub trait PrinterConnection: Send + Sync {
    async fn probe(&self) -> Result<ProbeResult, ConnectionError>;

    async fn subscribe(
        &self,
        tx: tokio::sync::mpsc::Sender<PrinterStatus>,
    ) -> Result<(), ConnectionError>;
}
```

- [ ] **Step 6: Register the module**

In `src-tauri/src/lib.rs`, add below `pub mod catalog;`:

```rust
pub mod connections;
```

Create an empty placeholder so the `pub mod credentials;` declaration resolves — Task 2 fills it in:

```sh
touch src-tauri/src/connections/credentials.rs
```

- [ ] **Step 7: Run the tests to verify they pass**

Run: `source "$HOME/.cargo/env" && cargo test --manifest-path src-tauri/Cargo.toml connections::`
Expected: 4 passed.

- [ ] **Step 8: Write the failing test for the typed `connection` slot**

Phase 1 stored `connection` as an opaque `serde_json::Value` precisely so this task could type it without a schema migration. Add to `src-tauri/src/printers.rs`'s existing `mod tests`:

```rust
#[test]
fn printers_file_without_a_connection_key_still_loads() {
    // Every printer written by phase 1 looks like this. Typing the slot must
    // not orphan them.
    let dir = temp_dir();
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        printers_file_path(&dir),
        r#"{"schemaVersion":1,"printers":[{"id":"prn-1","name":"Bay 1"}]}"#,
    )
    .unwrap();
    let loaded = load_printers_from(&dir).unwrap();
    assert_eq!(loaded.printers.len(), 1);
    assert_eq!(loaded.printers[0].connection, None);
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_stored_connection_round_trips_and_never_carries_a_secret() {
    use crate::connections::{ConnectionConfig, DEFAULT_MOONRAKER_PORT, MOONRAKER_KIND};

    let dir = temp_dir();
    let mut file = PrintersFile {
        schema_version: 1,
        printers: vec![StoredPrinter {
            id: "prn-1".to_string(),
            name: "Bay 1".to_string(),
            ..Default::default()
        }],
    };
    file.printers[0].connection = Some(ConnectionConfig {
        kind: MOONRAKER_KIND.to_string(),
        host: "voron.local".to_string(),
        port: DEFAULT_MOONRAKER_PORT,
        use_tls: false,
        credential_ref: Some("farm3d/printer/prn-1/apikey".to_string()),
    });
    write_printers_to(&dir, &file).unwrap();

    let raw = fs::read_to_string(printers_file_path(&dir)).unwrap();
    // The reference is stored; the secret it points at never is.
    assert!(raw.contains("credentialRef"));
    assert!(!raw.contains("apiKey\""));
    assert_eq!(load_printers_from(&dir).unwrap(), file);
    fs::remove_dir_all(&dir).ok();
}
```

- [ ] **Step 9: Run it to verify it fails**

Run: `source "$HOME/.cargo/env" && cargo test --manifest-path src-tauri/Cargo.toml printers::`
Expected: FAIL — a type mismatch assigning `ConnectionConfig` to an `Option<serde_json::Value>` field.

- [ ] **Step 10: Type the slot in both structs**

In `src-tauri/src/printers.rs`, replace the `connection` field on `StoredPrinter`:

```rust
    /// Phase 2's Connection config. Holds a `credentialRef` only — the
    /// secret it names lives in the OS keychain (or the 0600 fallback file),
    /// never here.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection: Option<crate::connections::ConnectionConfig>,
```

In `src-tauri/src/catalog/resolve.rs`, replace the matching field on `ResolvedPrinter`:

```rust
    pub connection: Option<crate::connections::ConnectionConfig>,
```

`resolve.rs:208`'s `connection: stored.connection.clone()` needs no change — it was always a passthrough.

- [ ] **Step 11: Run the whole Rust suite**

Run: `source "$HOME/.cargo/env" && cargo test --manifest-path src-tauri/Cargo.toml`
Expected: all pass. If any `resolve.rs` test constructs `connection: None`, it still compiles — `None` is type-inferred.

- [ ] **Step 12: Commit**

```bash
git add src-tauri/Cargo.toml src-tauri/Cargo.lock src-tauri/src/connections/ src-tauri/src/lib.rs src-tauri/src/printers.rs src-tauri/src/catalog/resolve.rs
git commit -m "feat: add connection types and the PrinterConnection trait"
```

---

### Task 2: Two-tier credential storage

The keychain, with a fallback that is **specified here rather than discovered during implementation** — that was an explicit spec requirement, because a Linux box with no Secret Service is a real deployment and silently downgrading a user's secret storage is not acceptable.

**Files:**
- Modify: `src-tauri/src/connections/credentials.rs` (created empty in Task 1)

**Interfaces:**
- Consumes: nothing from Task 1 but the module tree.
- Produces: `CredentialStore`, `CredentialStoreKind`, `credential_ref_for(printer_id) -> String`, `CredentialStore::detect(config_dir) -> CredentialStore`, `.set(&str, &str)`, `.get(&str) -> Result<Option<String>, String>`, `.delete(&str)`, `.kind()`.

- [ ] **Step 1: Write the failing tests**

Note what is and is not tested here. The **file tier is fully tested** because it is ours. The **keychain tier is not** — writing to the developer's real login keychain from `cargo test` would be a side effect on their machine, and mocking `keyring` would test the mock. Instead, `detect()` is tested for its decision, and the keychain path is exercised by the manual verification at the end of this plan.

Write into `src-tauri/src/connections/credentials.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn temp_dir() -> PathBuf {
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!("farm3d-cred-test-{}-{}", std::process::id(), id))
    }

    fn file_store(dir: &Path) -> CredentialStore {
        CredentialStore::file_backed(dir.to_path_buf())
    }

    #[test]
    fn credential_ref_is_namespaced_per_printer() {
        assert_eq!(credential_ref_for("prn-8f2a"), "farm3d/printer/prn-8f2a/apikey");
    }

    #[test]
    fn file_tier_round_trips_a_secret() {
        let dir = temp_dir();
        let store = file_store(&dir);
        let key = credential_ref_for("prn-1");
        store.set(&key, "s3cret").unwrap();
        assert_eq!(store.get(&key).unwrap(), Some("s3cret".to_string()));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn file_tier_returns_none_for_an_unknown_key() {
        let dir = temp_dir();
        let store = file_store(&dir);
        assert_eq!(store.get("farm3d/printer/nope/apikey").unwrap(), None);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn file_tier_deletes_without_disturbing_siblings() {
        let dir = temp_dir();
        let store = file_store(&dir);
        store.set(&credential_ref_for("a"), "aaa").unwrap();
        store.set(&credential_ref_for("b"), "bbb").unwrap();
        store.delete(&credential_ref_for("a")).unwrap();
        assert_eq!(store.get(&credential_ref_for("a")).unwrap(), None);
        assert_eq!(store.get(&credential_ref_for("b")).unwrap(), Some("bbb".to_string()));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn deleting_a_missing_key_is_not_an_error() {
        // Clearing a connection that never had a credential must not fail.
        let dir = temp_dir();
        let store = file_store(&dir);
        assert!(store.delete(&credential_ref_for("ghost")).is_ok());
        fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn file_tier_is_owner_only_and_stays_that_way() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir();
        let store = file_store(&dir);
        let key = credential_ref_for("prn-1");
        store.set(&key, "first").unwrap();

        let path = credentials_file_path(&dir);
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);

        // A user (or a bad backup restore) loosening the mode must be
        // corrected on the next write, not merely on creation.
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        store.set(&key, "second").unwrap();
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_corrupt_credentials_file_does_not_take_down_the_app() {
        // Unlike printers.json, a lost API key is re-enterable — so this
        // reports an error rather than quarantining, and the caller surfaces
        // it in the Connection tab.
        let dir = temp_dir();
        fs::create_dir_all(&dir).unwrap();
        fs::write(credentials_file_path(&dir), "not json").unwrap();
        assert!(file_store(&dir).get(&credential_ref_for("prn-1")).is_err());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn detect_reports_which_tier_is_live() {
        // Whichever tier this machine lands on, `kind()` must name it — the
        // spec's requirement is that farm3d never downgrades silently.
        let dir = temp_dir();
        let store = CredentialStore::detect(dir.clone());
        assert!(matches!(
            store.kind(),
            CredentialStoreKind::Keychain | CredentialStoreKind::File
        ));
        fs::remove_dir_all(&dir).ok();
    }
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `source "$HOME/.cargo/env" && cargo test --manifest-path src-tauri/Cargo.toml credentials::`
Expected: FAIL — `cannot find type CredentialStore in this scope`.

- [ ] **Step 3: Implement the store**

Prepend to `src-tauri/src/connections/credentials.rs`:

```rust
//! Two-tier secret storage for Connection credentials.
//!
//! Tier 1 is the platform secret store (Keychain Services / Credential
//! Manager / Secret Service) via `keyring`. Tier 2 is a `credentials.json`
//! beside `printers.json`, owner-readable only.
//!
//! The fallback exists because `keyring` needs a running Secret Service on
//! *nix, and a headless or minimal box may have none. It is deliberately
//! specified rather than discovered: farm3d reports which tier is live
//! (see `CredentialStoreKind`) rather than silently downgrading where a
//! user's API key lives.
//!
//! Under NO tier does a secret enter `printers.json`, which stores only the
//! `credentialRef` naming it.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const CREDENTIALS_FILE_NAME: &str = "credentials.json";
/// `keyring`'s "service" argument. The per-printer part goes in the username.
const KEYCHAIN_SERVICE: &str = "farm3d";

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum CredentialStoreKind {
    Keychain,
    File,
}

/// The stable name for one printer's secret. Also the exact string written
/// into `printers.json` as `credentialRef`, so it must stay stable across
/// releases — changing this format orphans every stored credential.
pub fn credential_ref_for(printer_id: &str) -> String {
    format!("farm3d/printer/{printer_id}/apikey")
}

pub fn credentials_file_path(config_dir: &Path) -> PathBuf {
    config_dir.join(CREDENTIALS_FILE_NAME)
}

pub struct CredentialStore {
    kind: CredentialStoreKind,
    config_dir: PathBuf,
    /// Why the keychain was unavailable, for the Connection tab to explain.
    unavailable_reason: Option<String>,
}

impl CredentialStore {
    /// Picks a tier once. `Entry::store_status()` initializes the platform
    /// store lazily and reports the outcome WITHOUT writing anything — which
    /// is exactly the availability check this needs.
    pub fn detect(config_dir: PathBuf) -> Self {
        match keyring::Entry::store_status() {
            Ok(()) => Self { kind: CredentialStoreKind::Keychain, config_dir, unavailable_reason: None },
            Err(e) => Self {
                kind: CredentialStoreKind::File,
                config_dir,
                unavailable_reason: Some(e.to_string()),
            },
        }
    }

    /// Forces the file tier. Used by tests, and the only constructor that
    /// never touches the developer's real keychain.
    pub fn file_backed(config_dir: PathBuf) -> Self {
        Self { kind: CredentialStoreKind::File, config_dir, unavailable_reason: None }
    }

    pub fn kind(&self) -> CredentialStoreKind {
        self.kind
    }

    pub fn unavailable_reason(&self) -> Option<&str> {
        self.unavailable_reason.as_deref()
    }

    pub fn set(&self, key: &str, secret: &str) -> Result<(), String> {
        match self.kind {
            CredentialStoreKind::Keychain => keychain_entry(key)?
                .set_password(secret)
                .map_err(|e| e.to_string()),
            CredentialStoreKind::File => {
                let mut map = self.read_file()?;
                map.insert(key.to_string(), secret.to_string());
                self.write_file(&map)
            }
        }
    }

    pub fn get(&self, key: &str) -> Result<Option<String>, String> {
        match self.kind {
            CredentialStoreKind::Keychain => match keychain_entry(key)?.get_password() {
                Ok(secret) => Ok(Some(secret)),
                Err(keyring::Error::NoEntry) => Ok(None),
                Err(e) => Err(e.to_string()),
            },
            CredentialStoreKind::File => Ok(self.read_file()?.get(key).cloned()),
        }
    }

    pub fn delete(&self, key: &str) -> Result<(), String> {
        match self.kind {
            CredentialStoreKind::Keychain => match keychain_entry(key)?.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(e) => Err(e.to_string()),
            },
            CredentialStoreKind::File => {
                let mut map = self.read_file()?;
                // Deleting a key that was never set is a success, not an
                // error — clearing a connection that had no credential is a
                // normal action.
                if map.remove(key).is_none() {
                    return Ok(());
                }
                self.write_file(&map)
            }
        }
    }

    fn read_file(&self) -> Result<BTreeMap<String, String>, String> {
        let path = credentials_file_path(&self.config_dir);
        if !path.exists() {
            return Ok(BTreeMap::new());
        }
        let contents = fs::read_to_string(&path).map_err(|e| e.to_string())?;
        serde_json::from_str(&contents).map_err(|e| {
            format!("{CREDENTIALS_FILE_NAME} is not readable ({e}); re-enter the credential to rewrite it")
        })
    }

    fn write_file(&self, map: &BTreeMap<String, String>) -> Result<(), String> {
        fs::create_dir_all(&self.config_dir).map_err(|e| e.to_string())?;
        let path = credentials_file_path(&self.config_dir);
        let json = serde_json::to_string_pretty(map).map_err(|e| e.to_string())?;
        fs::write(&path, json).map_err(|e| e.to_string())?;
        restrict_permissions(&path)
    }
}

fn keychain_entry(key: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(KEYCHAIN_SERVICE, key).map_err(|e| e.to_string())
}

/// Re-applied on EVERY write, not just on creation — a loosened mode from a
/// hand-edit or a restored backup must be corrected, not inherited.
#[cfg(unix)]
fn restrict_permissions(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|e| e.to_string())
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) -> Result<(), String> {
    // Windows inherits the user-profile ACL on the app config dir, which is
    // already owner-only; there is no chmod equivalent worth emulating.
    Ok(())
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `source "$HOME/.cargo/env" && cargo test --manifest-path src-tauri/Cargo.toml credentials::`
Expected: 8 passed (7 on non-Unix).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/connections/credentials.rs
git commit -m "feat: add two-tier credential storage with a keychain fallback"
```

---

### Task 3: Moonraker protocol codec (pure, no sockets)

**This is the highest-risk task in the phase.** Moonraker sends *partial* status updates; an adapter that treats each one as a snapshot blanks the bed temperature every time the nozzle ticks. Getting it right here — in a module with no I/O — means it is testable with plain `#[test]` functions over JSON literals.

**Files:**
- Create: `src-tauri/src/connections/moonraker/protocol.rs`
- Create: `src-tauri/src/connections/moonraker/mod.rs` (declaration only this task; Task 4 fills it)
- Modify: `src-tauri/src/connections/mod.rs` (add `pub mod moonraker;`)

**Interfaces:**
- Consumes: `PrinterStatus`, `ConnectionState`, `ProbeResult`, `ReportedCapabilities`, `ConnectionError` from Task 1.
- Produces: `rpc_request(id, method, params) -> String`, `subscribe_params() -> serde_json::Value`, `parse_frame(&str) -> Frame`, `Frame`, `StatusSnapshot` (with `merge`, `to_status`), `probe_result_from(server_info, printer_info, snapshot) -> ProbeResult`.

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot_with_both_heaters() -> StatusSnapshot {
        let mut snapshot = StatusSnapshot::default();
        snapshot.merge(&serde_json::json!({
            "extruder": {"temperature": 200.5, "target": 210.0},
            "heater_bed": {"temperature": 60.1, "target": 60.0},
            "print_stats": {"filename": "benchy.gcode", "state": "printing", "print_duration": 42.0},
            "display_status": {"progress": 0.25}
        }));
        snapshot
    }

    #[test]
    fn a_partial_update_does_not_blank_the_other_fields() {
        // THE bug this module exists to prevent. Moonraker sends only what
        // changed; treating a notification as a whole snapshot wipes every
        // reading it omits.
        let mut snapshot = snapshot_with_both_heaters();
        snapshot.merge(&serde_json::json!({"extruder": {"temperature": 201.4}}));

        let status = snapshot.to_status(ConnectionState::Online);
        assert_eq!(status.nozzle_temp_c, Some(201.4));
        assert_eq!(status.bed_temp_c, Some(60.1), "bed temperature was blanked by a nozzle-only update");
        assert_eq!(status.nozzle_target_c, Some(210.0), "nozzle target was blanked by a temperature-only update");
        assert_eq!(status.job_name, Some("benchy.gcode".to_string()));
        assert_eq!(status.progress, Some(0.25));
    }

    #[test]
    fn merging_an_unknown_object_is_ignored_rather_than_fatal() {
        // Moonraker instances expose different printer objects; an adapter
        // that errors on an unexpected key is an adapter that breaks on the
        // next Klipper release.
        let mut snapshot = snapshot_with_both_heaters();
        snapshot.merge(&serde_json::json!({"fan": {"speed": 1.0}, "mcu": {"last_stats": {}}}));
        assert_eq!(snapshot.to_status(ConnectionState::Online).bed_temp_c, Some(60.1));
    }

    #[test]
    fn an_empty_snapshot_reports_no_readings_rather_than_zeroes() {
        let status = StatusSnapshot::default().to_status(ConnectionState::Connecting);
        assert_eq!(status.nozzle_temp_c, None);
        assert_eq!(status.bed_temp_c, None);
        assert_eq!(status.job_state, None);
        assert_eq!(status.connection_state, ConnectionState::Connecting);
    }

    #[test]
    fn parses_a_status_notification_with_positional_params() {
        // Moonraker ALWAYS sends notification params as an array — first the
        // status object, then a Klipper-uptime float. Parsing it as a named
        // object silently matches nothing.
        let frame = parse_frame(
            r#"{"jsonrpc":"2.0","method":"notify_status_update",
                "params":[{"extruder":{"temperature":201.4}},578243.578]}"#,
        );
        match frame {
            Frame::StatusUpdate(status) => {
                assert_eq!(status["extruder"]["temperature"], 201.4);
            }
            other => panic!("expected a StatusUpdate, got {other:?}"),
        }
    }

    #[test]
    fn parses_klippy_lifecycle_notifications_that_carry_no_params() {
        assert!(matches!(
            parse_frame(r#"{"jsonrpc":"2.0","method":"notify_klippy_ready"}"#),
            Frame::KlippyReady
        ));
        assert!(matches!(
            parse_frame(r#"{"jsonrpc":"2.0","method":"notify_klippy_shutdown"}"#),
            Frame::KlippyDown
        ));
        assert!(matches!(
            parse_frame(r#"{"jsonrpc":"2.0","method":"notify_klippy_disconnected"}"#),
            Frame::KlippyDown
        ));
    }

    #[test]
    fn parses_a_response_and_an_error_by_id() {
        match parse_frame(r#"{"jsonrpc":"2.0","result":{"klippy_state":"ready"},"id":1}"#) {
            Frame::Response { id, result } => {
                assert_eq!(id, 1);
                assert_eq!(result["klippy_state"], "ready");
            }
            other => panic!("expected a Response, got {other:?}"),
        }
        match parse_frame(r#"{"jsonrpc":"2.0","error":{"code":401,"message":"Unauthorized"},"id":2}"#) {
            Frame::Error { id, message, code } => {
                assert_eq!(id, 2);
                assert_eq!(code, Some(401));
                assert!(message.contains("Unauthorized"));
            }
            other => panic!("expected an Error, got {other:?}"),
        }
    }

    #[test]
    fn unrecognized_frames_are_ignored_not_errors() {
        assert!(matches!(parse_frame(r#"{"jsonrpc":"2.0","method":"notify_gcode_response","params":["ok"]}"#), Frame::Ignored));
        assert!(matches!(parse_frame("this is not json"), Frame::Ignored));
    }

    #[test]
    fn subscribe_params_request_only_the_fields_we_render() {
        let params = subscribe_params();
        let objects = &params["objects"];
        assert!(objects.get("extruder").is_some());
        assert!(objects.get("heater_bed").is_some());
        assert!(objects.get("print_stats").is_some());
        assert!(objects.get("display_status").is_some());
        assert!(objects.get("toolhead").is_some());
        // Narrow subscriptions keep the notification volume down; `null`
        // here would subscribe to every attribute of every object.
        assert!(objects["extruder"].is_array());
    }

    #[test]
    fn rpc_request_is_well_formed_json_rpc() {
        let raw = rpc_request(7, "printer.info", None);
        let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed["jsonrpc"], "2.0");
        assert_eq!(parsed["method"], "printer.info");
        assert_eq!(parsed["id"], 7);
        assert!(parsed.get("params").is_none(), "a params-less call must omit params entirely");
    }

    #[test]
    fn probe_reads_the_build_volume_from_axis_limits() {
        let server_info = serde_json::json!({"klippy_state": "ready", "moonraker_version": "v0.9.3"});
        let printer_info = serde_json::json!({
            "hostname": "voron", "software_version": "v0.12.0-85-gd785b396",
            "state": "ready", "state_message": "Printer is ready"
        });
        let mut snapshot = StatusSnapshot::default();
        // 4-element [x, y, z, e] arrays; the 4th is always zero and formally
        // deprecated, so only indices 0-2 may be read.
        snapshot.merge(&serde_json::json!({"toolhead": {
            "axis_minimum": [0.0, -4.0, -2.0, 0.0],
            "axis_maximum": [250.0, 210.0, 220.0, 0.0]
        }}));

        let probe = probe_result_from(&server_info, &printer_info, &snapshot);
        assert_eq!(probe.kind, "moonraker");
        assert_eq!(probe.host_software, "v0.9.3");
        assert_eq!(probe.firmware, "v0.12.0-85-gd785b396");
        assert_eq!(probe.reported_name, "voron");
        assert_eq!(probe.state, "ready");
        assert_eq!(probe.reported.bed_width_mm, Some(250.0));
        assert_eq!(probe.reported.bed_depth_mm, Some(214.0)); // 210 - (-4)
        assert_eq!(probe.reported.printable_height_mm, Some(222.0)); // 220 - (-2)
    }

    #[test]
    fn probe_tolerates_a_printer_that_reports_no_axis_limits() {
        // A shut-down or unhomed Klipper reports no toolhead limits. That is
        // a normal state, not a probe failure.
        let probe = probe_result_from(
            &serde_json::json!({"klippy_state": "shutdown"}),
            &serde_json::json!({"state_message": "Klipper is shut down"}),
            &StatusSnapshot::default(),
        );
        assert_eq!(probe.reported, ReportedCapabilities::default());
        assert_eq!(probe.state, "shutdown");
        assert_eq!(probe.reported_name, "");
    }
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `source "$HOME/.cargo/env" && cargo test --manifest-path src-tauri/Cargo.toml moonraker::protocol`
Expected: FAIL — `cannot find function parse_frame in this scope`.

- [ ] **Step 3: Implement the codec**

Prepend to `src-tauri/src/connections/moonraker/protocol.rs`:

```rust
//! Moonraker's JSON-RPC 2.0 framing and status accumulation — PURE. No
//! sockets, no async, no Tauri. Everything here is a function of its input,
//! which is what makes the merge semantics below testable without a printer.

use crate::connections::{
    ConnectionState, PrinterStatus, ProbeResult, ReportedCapabilities,
};
use serde_json::{json, Value};

/// One decoded inbound frame. Anything farm3d does not act on decodes to
/// `Ignored` rather than an error: Moonraker emits many notifications
/// (`notify_gcode_response`, `notify_proc_stat_update`, …) that are not this
/// phase's business, and an adapter that errors on them is an adapter that
/// disconnects constantly.
#[derive(Debug)]
pub enum Frame {
    Response { id: u64, result: Value },
    Error { id: u64, message: String, code: Option<i64> },
    StatusUpdate(Value),
    KlippyReady,
    KlippyDown,
    Ignored,
}

pub fn rpc_request(id: u64, method: &str, params: Option<Value>) -> String {
    let mut req = json!({"jsonrpc": "2.0", "method": method, "id": id});
    if let Some(params) = params {
        req["params"] = params;
    }
    req.to_string()
}

/// Exactly the objects and attributes the dashboard renders. Requesting
/// named attributes rather than `null` (which means "every attribute") keeps
/// the notification volume proportional to what is actually displayed.
pub fn subscribe_params() -> Value {
    json!({"objects": {
        "extruder": ["temperature", "target"],
        "heater_bed": ["temperature", "target"],
        "print_stats": ["filename", "state", "print_duration", "message"],
        "display_status": ["progress"],
        "toolhead": ["axis_minimum", "axis_maximum"]
    }})
}

pub fn parse_frame(raw: &str) -> Frame {
    let Ok(value) = serde_json::from_str::<Value>(raw) else {
        return Frame::Ignored;
    };

    if let Some(method) = value.get("method").and_then(Value::as_str) {
        return match method {
            // Params are POSITIONAL: [status_object, eventtime]. Moonraker
            // documents that notification params are always an array.
            "notify_status_update" => value
                .get("params")
                .and_then(Value::as_array)
                .and_then(|p| p.first())
                .cloned()
                .map(Frame::StatusUpdate)
                .unwrap_or(Frame::Ignored),
            "notify_klippy_ready" => Frame::KlippyReady,
            // Both mean "the firmware is not usable right now" even though
            // our socket to Moonraker is perfectly healthy.
            "notify_klippy_shutdown" | "notify_klippy_disconnected" => Frame::KlippyDown,
            _ => Frame::Ignored,
        };
    }

    let Some(id) = value.get("id").and_then(Value::as_u64) else {
        return Frame::Ignored;
    };
    if let Some(error) = value.get("error") {
        return Frame::Error {
            id,
            message: error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown error")
                .to_string(),
            code: error.get("code").and_then(Value::as_i64),
        };
    }
    match value.get("result") {
        Some(result) => Frame::Response { id, result: result.clone() },
        None => Frame::Ignored,
    }
}

/// The running merge of every status object Moonraker has reported.
///
/// Moonraker's `notify_status_update` carries ONLY the fields that changed.
/// This type is the reason that is safe: each notification is folded into the
/// accumulated tree, per-object and per-attribute, and `to_status` reads the
/// accumulated result. Replacing the tree per notification is the bug.
#[derive(Default, Debug, Clone)]
pub struct StatusSnapshot {
    objects: serde_json::Map<String, Value>,
}

impl StatusSnapshot {
    pub fn merge(&mut self, update: &Value) {
        let Some(update) = update.as_object() else { return };
        for (object_name, attributes) in update {
            let Some(attributes) = attributes.as_object() else { continue };
            let entry = self
                .objects
                .entry(object_name.clone())
                .or_insert_with(|| Value::Object(serde_json::Map::new()));
            let Some(entry) = entry.as_object_mut() else { continue };
            for (attribute, value) in attributes {
                entry.insert(attribute.clone(), value.clone());
            }
        }
    }

    fn number(&self, object: &str, attribute: &str) -> Option<f64> {
        self.objects.get(object)?.get(attribute)?.as_f64()
    }

    fn string(&self, object: &str, attribute: &str) -> Option<String> {
        Some(self.objects.get(object)?.get(attribute)?.as_str()?.to_string())
    }

    /// Reads one axis extent. The arrays are `[x, y, z, e]` where the 4th
    /// element is always zero and formally deprecated — index by position,
    /// never by length.
    fn axis_span(&self, index: usize) -> Option<f64> {
        let toolhead = self.objects.get("toolhead")?;
        let min = toolhead.get("axis_minimum")?.as_array()?.get(index)?.as_f64()?;
        let max = toolhead.get("axis_maximum")?.as_array()?.get(index)?.as_f64()?;
        Some(max - min)
    }

    pub fn reported_capabilities(&self) -> ReportedCapabilities {
        ReportedCapabilities {
            bed_width_mm: self.axis_span(0),
            bed_depth_mm: self.axis_span(1),
            printable_height_mm: self.axis_span(2),
        }
    }

    pub fn to_status(&self, connection_state: ConnectionState) -> PrinterStatus {
        let mut status = PrinterStatus::new(connection_state);
        status.nozzle_temp_c = self.number("extruder", "temperature");
        status.nozzle_target_c = self.number("extruder", "target");
        status.bed_temp_c = self.number("heater_bed", "temperature");
        status.bed_target_c = self.number("heater_bed", "target");
        status.job_state = self.string("print_stats", "state");
        status.job_name = self.string("print_stats", "filename");
        status.print_duration_s = self.number("print_stats", "print_duration");
        status.progress = self.number("display_status", "progress");
        status
    }
}

fn str_field(value: &Value, key: &str) -> String {
    value.get(key).and_then(Value::as_str).unwrap_or_default().to_string()
}

pub fn probe_result_from(
    server_info: &Value,
    printer_info: &Value,
    snapshot: &StatusSnapshot,
) -> ProbeResult {
    ProbeResult {
        kind: crate::connections::MOONRAKER_KIND.to_string(),
        host_software: str_field(server_info, "moonraker_version"),
        firmware: str_field(printer_info, "software_version"),
        reported_name: str_field(printer_info, "hostname"),
        state: str_field(server_info, "klippy_state"),
        state_message: str_field(printer_info, "state_message"),
        reported: snapshot.reported_capabilities(),
    }
}
```

- [ ] **Step 4: Declare the module**

Create `src-tauri/src/connections/moonraker/mod.rs` with just:

```rust
pub mod protocol;
```

And add to `src-tauri/src/connections/mod.rs`, beside `pub mod credentials;`:

```rust
pub mod moonraker;
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `source "$HOME/.cargo/env" && cargo test --manifest-path src-tauri/Cargo.toml moonraker::protocol`
Expected: 11 passed.

- [ ] **Step 6: Prove the partial-merge test actually catches the bug**

This test is the whole point of the task, so confirm it fails against the wrong implementation before trusting it. Temporarily replace the body of `StatusSnapshot::merge` with the naive version:

```rust
    pub fn merge(&mut self, update: &Value) {
        // WRONG ON PURPOSE — replaces instead of folding.
        if let Some(update) = update.as_object() {
            self.objects = update.clone();
        }
    }
```

Run: `source "$HOME/.cargo/env" && cargo test --manifest-path src-tauri/Cargo.toml moonraker::protocol`
Expected: FAIL on `a_partial_update_does_not_blank_the_other_fields` with "bed temperature was blanked by a nozzle-only update".

Then restore the real implementation and re-run — expected: 11 passed.

- [ ] **Step 7: Commit**

```bash
git add src-tauri/src/connections/moonraker/ src-tauri/src/connections/mod.rs
git commit -m "feat: add Moonraker JSON-RPC framing and partial-update status merging"
```

---

### Task 4: The Moonraker adapter

The first code in this repo that opens a socket. It stays thin because Task 3 owns the framing: this task is connect, send, read, dispatch.

**Files:**
- Modify: `src-tauri/src/connections/moonraker/mod.rs`

**Interfaces:**
- Consumes: `PrinterConnection`, `ConnectionConfig`, `ConnectionError`, `PrinterStatus`, `ConnectionState`, `ProbeResult` (Task 1); `rpc_request`, `subscribe_params`, `parse_frame`, `Frame`, `StatusSnapshot`, `probe_result_from` (Task 3).
- Produces: `MoonrakerConnection::new(config: ConnectionConfig, api_key: Option<String>) -> Self`, `websocket_url(&ConnectionConfig) -> String`, and the `impl PrinterConnection for MoonrakerConnection`.

- [ ] **Step 1: Write the failing tests**

Only the pure helpers are unit-tested. `probe()` and `subscribe()` open real sockets; testing them needs a live Moonraker, which the end-to-end verification at the bottom of this plan covers. Do **not** add a mock WebSocket server — it would test the mock, and Task 3 already covers every decision that isn't "did the bytes move".

Append to `src-tauri/src/connections/moonraker/mod.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::connections::{DEFAULT_MOONRAKER_PORT, MOONRAKER_KIND};

    fn config(use_tls: bool) -> ConnectionConfig {
        ConnectionConfig {
            kind: MOONRAKER_KIND.to_string(),
            host: "voron.local".to_string(),
            port: DEFAULT_MOONRAKER_PORT,
            use_tls,
            credential_ref: None,
        }
    }

    #[test]
    fn builds_a_plain_websocket_url() {
        assert_eq!(websocket_url(&config(false)), "ws://voron.local:7125/websocket");
    }

    #[test]
    fn builds_a_tls_websocket_url() {
        assert_eq!(websocket_url(&config(true)), "wss://voron.local:7125/websocket");
    }

    #[test]
    fn an_api_key_becomes_an_upgrade_request_header() {
        let request = upgrade_request(&config(false), Some("abc123")).unwrap();
        assert_eq!(request.headers()["X-Api-Key"], "abc123");
    }

    #[test]
    fn no_api_key_means_no_header() {
        // Moonraker instances on a trusted LAN need no key at all; sending an
        // empty one would be rejected where sending none is accepted.
        let request = upgrade_request(&config(false), None).unwrap();
        assert!(request.headers().get("X-Api-Key").is_none());
    }
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `source "$HOME/.cargo/env" && cargo test --manifest-path src-tauri/Cargo.toml moonraker::tests`
Expected: FAIL — `cannot find function websocket_url in this scope`.

- [ ] **Step 3: Implement the adapter**

Prepend to `src-tauri/src/connections/moonraker/mod.rs`, above `pub mod protocol;`:

```rust
//! The Moonraker adapter: JSON-RPC 2.0 over `ws://<host>:<port>/websocket`.
//!
//! Deliberately thin. All framing and all status accumulation live in
//! `protocol`, which has no I/O and therefore real test coverage. What is
//! left here is connect, send, read, dispatch.
//!
//! This adapter implements NO retry logic. The supervisor owns
//! reconnect-with-backoff; `subscribe` returning at all is the supervisor's
//! cue to reconnect.

use crate::connections::{
    ConnectionConfig, ConnectionError, ConnectionState, PrinterConnection, PrinterStatus,
    ProbeResult,
};
use futures_util::{SinkExt, StreamExt};
use protocol::{parse_frame, probe_result_from, rpc_request, subscribe_params, Frame, StatusSnapshot};
use std::time::Duration;
use tokio::sync::mpsc::Sender;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::handshake::client::Request;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message;

pub mod protocol;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

const ID_SERVER_INFO: u64 = 1;
const ID_PRINTER_INFO: u64 = 2;
const ID_SUBSCRIBE: u64 = 3;

pub struct MoonrakerConnection {
    config: ConnectionConfig,
    api_key: Option<String>,
}

impl MoonrakerConnection {
    pub fn new(config: ConnectionConfig, api_key: Option<String>) -> Self {
        Self { config, api_key }
    }
}

pub fn websocket_url(config: &ConnectionConfig) -> String {
    let scheme = if config.use_tls { "wss" } else { "ws" };
    format!("{scheme}://{}:{}/websocket", config.host, config.port)
}

/// Moonraker accepts the API key as a header on the WebSocket upgrade, which
/// avoids the `/access/oneshot_token` round trip entirely — and with it, any
/// need for an HTTP client in this phase.
pub fn upgrade_request(
    config: &ConnectionConfig,
    api_key: Option<&str>,
) -> Result<Request, ConnectionError> {
    let mut request = websocket_url(config)
        .into_client_request()
        .map_err(|e| ConnectionError::Unreachable(e.to_string()))?;
    if let Some(key) = api_key.filter(|k| !k.is_empty()) {
        let value = HeaderValue::from_str(key)
            .map_err(|_| ConnectionError::Auth("the API key contains invalid characters".into()))?;
        request.headers_mut().insert("X-Api-Key", value);
    }
    Ok(request)
}

type Socket = tokio_tungstenite::WebSocketStream<
    tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
>;

async fn connect(
    config: &ConnectionConfig,
    api_key: Option<&str>,
) -> Result<Socket, ConnectionError> {
    let request = upgrade_request(config, api_key)?;
    let connecting = tokio_tungstenite::connect_async(request);
    match tokio::time::timeout(CONNECT_TIMEOUT, connecting).await {
        Err(_) => Err(ConnectionError::Timeout),
        Ok(Err(e)) => Err(classify(e)),
        Ok(Ok((socket, _response))) => Ok(socket),
    }
}

/// Separating auth failure from unreachability is what lets the Connection
/// tab say "check the API key" instead of "check the address".
fn classify(error: tokio_tungstenite::tungstenite::Error) -> ConnectionError {
    use tokio_tungstenite::tungstenite::Error as WsError;
    match &error {
        WsError::Http(response) if response.status().as_u16() == 401 || response.status().as_u16() == 403 => {
            ConnectionError::Auth(format!("HTTP {}", response.status()))
        }
        _ => ConnectionError::Unreachable(error.to_string()),
    }
}

async fn send(socket: &mut Socket, frame: String) -> Result<(), ConnectionError> {
    socket
        .send(Message::Text(frame.into()))
        .await
        .map_err(|e| ConnectionError::Unreachable(e.to_string()))
}

/// Reads until a frame we care about arrives, discarding the rest. Returns
/// `None` when the socket closes.
async fn next_frame(socket: &mut Socket) -> Option<Frame> {
    while let Some(message) = socket.next().await {
        match message {
            Ok(Message::Text(text)) => return Some(parse_frame(&text)),
            // Ping/Pong are answered by tungstenite; Binary and Close are not
            // part of Moonraker's JSON-RPC surface.
            Ok(Message::Close(_)) | Err(_) => return None,
            Ok(_) => continue,
        }
    }
    None
}

#[async_trait::async_trait]
impl PrinterConnection for MoonrakerConnection {
    async fn probe(&self) -> Result<ProbeResult, ConnectionError> {
        let mut socket = connect(&self.config, self.api_key.as_deref()).await?;
        send(&mut socket, rpc_request(ID_SERVER_INFO, "server.info", None)).await?;
        send(&mut socket, rpc_request(ID_PRINTER_INFO, "printer.info", None)).await?;
        send(
            &mut socket,
            rpc_request(ID_SUBSCRIBE, "printer.objects.query", Some(subscribe_params())),
        )
        .await?;

        let mut server_info = serde_json::Value::Null;
        let mut printer_info = serde_json::Value::Null;
        let mut snapshot = StatusSnapshot::default();
        let mut outstanding = 3;

        let collect = async {
            while outstanding > 0 {
                match next_frame(&mut socket).await {
                    None => {
                        return Err(ConnectionError::Protocol(
                            "the printer closed the connection before answering".into(),
                        ))
                    }
                    Some(Frame::Response { id, result }) => {
                        match id {
                            ID_SERVER_INFO => server_info = result,
                            ID_PRINTER_INFO => printer_info = result,
                            // `printer.objects.query` wraps its payload in a
                            // `status` key; the subscription notifications do
                            // not, which is why only this branch unwraps.
                            ID_SUBSCRIBE => snapshot.merge(&result["status"]),
                            _ => continue,
                        }
                        outstanding -= 1;
                    }
                    // `printer.info` fails outright when Klipper is down, but
                    // the probe is still a success: it proves we reached the
                    // right host, and `server.info.klippy_state` explains the
                    // rest. Reporting it as a connection failure would send a
                    // user hunting for a network problem they do not have.
                    Some(Frame::Error { id, message, code }) => {
                        if code == Some(401) || code == Some(403) {
                            return Err(ConnectionError::Auth(message));
                        }
                        if id == ID_PRINTER_INFO || id == ID_SUBSCRIBE {
                            outstanding -= 1;
                            continue;
                        }
                        return Err(ConnectionError::Protocol(message));
                    }
                    Some(_) => continue,
                }
            }
            Ok(())
        };

        match tokio::time::timeout(PROBE_TIMEOUT, collect).await {
            Err(_) => return Err(ConnectionError::Timeout),
            Ok(result) => result?,
        }

        let _ = socket.close(None).await;
        Ok(probe_result_from(&server_info, &printer_info, &snapshot))
    }

    async fn subscribe(&self, tx: Sender<PrinterStatus>) -> Result<(), ConnectionError> {
        let mut socket = connect(&self.config, self.api_key.as_deref()).await?;
        send(
            &mut socket,
            rpc_request(ID_SUBSCRIBE, "printer.objects.subscribe", Some(subscribe_params())),
        )
        .await?;

        let mut snapshot = StatusSnapshot::default();
        // Klipper can be down behind a perfectly healthy Moonraker socket, so
        // connection state is tracked from the lifecycle notifications rather
        // than assumed from the socket being open.
        let mut state = ConnectionState::Online;

        while let Some(frame) = next_frame(&mut socket).await {
            match frame {
                Frame::Response { id, result } if id == ID_SUBSCRIBE => {
                    snapshot.merge(&result["status"]);
                }
                Frame::StatusUpdate(update) => snapshot.merge(&update),
                Frame::KlippyReady => state = ConnectionState::Online,
                Frame::KlippyDown => state = ConnectionState::Offline,
                Frame::Error { message, code, .. } if code == Some(401) || code == Some(403) => {
                    return Err(ConnectionError::Auth(message));
                }
                _ => continue,
            }
            // A closed receiver means the supervisor dropped this printer.
            // Ending cleanly beats logging into the void.
            if tx.send(snapshot.to_status(state)).await.is_err() {
                return Ok(());
            }
        }
        Ok(())
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `source "$HOME/.cargo/env" && cargo test --manifest-path src-tauri/Cargo.toml moonraker`
Expected: 15 passed (11 from Task 3 + 4 here).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/connections/moonraker/mod.rs
git commit -m "feat: add the Moonraker WebSocket adapter"
```

---

### Task 5: The connection supervisor

One `tokio` task per connected Printer. Owns reconnect-with-backoff so no adapter has to, holds the last status for late-joining UI, and fans status out to the frontend as Tauri events.

**Files:**
- Create: `src-tauri/src/connections/supervisor.rs`
- Modify: `src-tauri/src/connections/mod.rs` (add `pub mod supervisor;`)

**Interfaces:**
- Consumes: everything from Tasks 1–4.
- Produces: `ConnectionManager::new(app: AppHandle) -> Self`, `.start(printer_id: String, config: ConnectionConfig, api_key: Option<String>)`, `.stop(printer_id: &str)`, `.statuses() -> HashMap<String, PrinterStatus>`, `backoff_delay(attempt: u32) -> Duration`, and the `printer-status` event name via `STATUS_EVENT`.

- [ ] **Step 1: Write the failing tests**

The supervisor's own loop needs a live printer, so what gets unit-tested is the part with a decision in it: the backoff schedule and the status map.

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_then_holds_at_a_ceiling() {
        assert_eq!(backoff_delay(0), Duration::from_secs(1));
        assert_eq!(backoff_delay(1), Duration::from_secs(2));
        assert_eq!(backoff_delay(2), Duration::from_secs(4));
        assert_eq!(backoff_delay(5), Duration::from_secs(32));
        // A printer that is simply switched off must not be retried every
        // second forever, nor drift toward never retrying at all.
        assert_eq!(backoff_delay(6), MAX_BACKOFF);
        assert_eq!(backoff_delay(99), MAX_BACKOFF);
    }

    #[test]
    fn backoff_never_overflows_on_a_long_outage() {
        // A printer offline for days reaches attempt counts that would panic
        // a naive `2u64.pow(attempt)` in debug builds.
        assert_eq!(backoff_delay(u32::MAX), MAX_BACKOFF);
    }

    #[test]
    fn the_status_map_starts_empty_and_records_per_printer() {
        let map = StatusMap::default();
        assert!(map.snapshot().is_empty());
        map.set("prn-1", PrinterStatus::new(ConnectionState::Online));
        map.set("prn-2", PrinterStatus::errored("nope"));
        let snapshot = map.snapshot();
        assert_eq!(snapshot["prn-1"].connection_state, ConnectionState::Online);
        assert_eq!(snapshot["prn-2"].connection_state, ConnectionState::Error);
    }

    #[test]
    fn forgetting_a_printer_drops_its_status() {
        let map = StatusMap::default();
        map.set("prn-1", PrinterStatus::new(ConnectionState::Online));
        map.forget("prn-1");
        assert!(map.snapshot().is_empty());
    }
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `source "$HOME/.cargo/env" && cargo test --manifest-path src-tauri/Cargo.toml supervisor::`
Expected: FAIL — `cannot find function backoff_delay in this scope`.

- [ ] **Step 3: Implement the supervisor**

Prepend to `src-tauri/src/connections/supervisor.rs`:

```rust
//! One long-lived task per connected Printer.
//!
//! The supervisor exists so no adapter implements retry. An adapter's
//! `subscribe` returning — cleanly or with an error — is this module's cue to
//! back off and reconnect. That keeps phase 3's adapters simple and keeps the
//! reconnect policy in exactly one place.

use super::moonraker::MoonrakerConnection;
use super::{ConnectionConfig, ConnectionState, PrinterConnection, PrinterStatus, MOONRAKER_KIND};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use tokio::task::JoinHandle;

/// Frontend event name. The payload is `{ id, status }`.
pub const STATUS_EVENT: &str = "printer-status";

const MAX_BACKOFF: Duration = Duration::from_secs(60);
/// Bounded so one wedged printer cannot stall the others behind a full queue.
const STATUS_CHANNEL_DEPTH: usize = 16;

/// 1s, 2s, 4s … capped at 60s. Saturating throughout: a printer that has been
/// off for days reaches attempt counts that overflow a naive shift.
pub fn backoff_delay(attempt: u32) -> Duration {
    let secs = 1u64.checked_shl(attempt).unwrap_or(u64::MAX);
    Duration::from_secs(secs).min(MAX_BACKOFF)
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct StatusEvent {
    id: String,
    status: PrinterStatus,
}

use serde::Serialize;

/// Last known status per printer, so a Connection tab opened after a printer
/// came online still renders live values instead of waiting for the next tick.
#[derive(Default)]
pub struct StatusMap(Mutex<HashMap<String, PrinterStatus>>);

impl StatusMap {
    pub fn set(&self, id: &str, status: PrinterStatus) {
        self.0.lock().unwrap().insert(id.to_string(), status);
    }

    pub fn forget(&self, id: &str) {
        self.0.lock().unwrap().remove(id);
    }

    pub fn snapshot(&self) -> HashMap<String, PrinterStatus> {
        self.0.lock().unwrap().clone()
    }
}

pub struct ConnectionManager {
    app: AppHandle,
    tasks: Mutex<HashMap<String, JoinHandle<()>>>,
    statuses: Arc<StatusMap>,
}

impl ConnectionManager {
    pub fn new(app: AppHandle) -> Self {
        Self { app, tasks: Mutex::new(HashMap::new()), statuses: Arc::new(StatusMap::default()) }
    }

    pub fn statuses(&self) -> HashMap<String, PrinterStatus> {
        self.statuses.snapshot()
    }

    /// Idempotent: starting a printer that is already running replaces its
    /// task, which is what a config edit needs.
    pub fn start(&self, printer_id: String, config: ConnectionConfig, api_key: Option<String>) {
        self.stop(&printer_id);

        let app = self.app.clone();
        let statuses = Arc::clone(&self.statuses);
        let id = printer_id.clone();

        let handle = tauri::async_runtime::spawn(async move {
            let mut attempt: u32 = 0;
            loop {
                publish(&app, &statuses, &id, PrinterStatus::new(ConnectionState::Connecting));

                let (tx, mut rx) = tokio::sync::mpsc::channel(STATUS_CHANNEL_DEPTH);
                let connection = build(&config, api_key.clone());

                let forward = {
                    let app = app.clone();
                    let statuses = Arc::clone(&statuses);
                    let id = id.clone();
                    tauri::async_runtime::spawn(async move {
                        while let Some(status) = rx.recv().await {
                            publish(&app, &statuses, &id, status);
                        }
                    })
                };

                let outcome = match connection {
                    Some(connection) => connection.subscribe(tx).await,
                    None => {
                        // An unknown `kind` is a phase-3 config read by a
                        // phase-2 build. Report it and stop — retrying a
                        // protocol we cannot speak never succeeds.
                        publish(
                            &app,
                            &statuses,
                            &id,
                            PrinterStatus::errored(format!(
                                "This build cannot speak `{}` connections",
                                config.kind
                            )),
                        );
                        forward.abort();
                        return;
                    }
                };
                forward.abort();

                match outcome {
                    Ok(()) => attempt = 0,
                    Err(e) => {
                        publish(&app, &statuses, &id, PrinterStatus::errored(e.to_string()));
                        attempt = attempt.saturating_add(1);
                    }
                }
                tokio::time::sleep(backoff_delay(attempt)).await;
            }
        });

        self.tasks.lock().unwrap().insert(printer_id, handle);
    }

    pub fn stop(&self, printer_id: &str) {
        if let Some(handle) = self.tasks.lock().unwrap().remove(printer_id) {
            handle.abort();
        }
        self.statuses.forget(printer_id);
    }
}

fn build(config: &ConnectionConfig, api_key: Option<String>) -> Option<Box<dyn PrinterConnection>> {
    match config.kind.as_str() {
        MOONRAKER_KIND => Some(Box::new(MoonrakerConnection::new(config.clone(), api_key))),
        _ => None,
    }
}

fn publish(app: &AppHandle, statuses: &StatusMap, id: &str, status: PrinterStatus) {
    statuses.set(id, status.clone());
    // A failed emit means the window is gone; the status map is still
    // correct, so there is nothing to recover from here.
    let _ = app.emit(STATUS_EVENT, StatusEvent { id: id.to_string(), status });
}
```

- [ ] **Step 4: Declare the module**

Add to `src-tauri/src/connections/mod.rs`:

```rust
pub mod supervisor;
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `source "$HOME/.cargo/env" && cargo test --manifest-path src-tauri/Cargo.toml supervisor::`
Expected: 4 passed.

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/connections/supervisor.rs src-tauri/src/connections/mod.rs
git commit -m "feat: add the connection supervisor with reconnect backoff"
```

---

### Task 6: mDNS discovery

An accelerator for finding printers, never a gate in front of manual entry.

**Files:**
- Create: `src-tauri/src/connections/discovery.rs`
- Modify: `src-tauri/src/connections/mod.rs` (add `pub mod discovery;`)

**Interfaces:**
- Produces: `DiscoveredPrinter { kind, name, host, port, addresses }`, `discover(window: Duration) -> Vec<DiscoveredPrinter>`, `kind_for_service(&str) -> Option<&'static str>`, `SERVICE_TYPES`.

- [ ] **Step 1: Write the failing tests**

Real mDNS browsing needs a real network with a real printer, so the tests cover the mapping decisions and the invariant that discovery is never load-bearing.

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_service_types_to_connection_kinds() {
        assert_eq!(kind_for_service("_moonraker._tcp.local."), Some("moonraker"));
        // Browsed now so phase 3's adapter needs no discovery changes.
        assert_eq!(kind_for_service("_octoprint._tcp.local."), Some("octoprint"));
        assert_eq!(kind_for_service("_http._tcp.local."), None);
    }

    #[test]
    fn browses_exactly_the_two_service_types_we_can_configure() {
        assert_eq!(SERVICE_TYPES.len(), 2);
        assert!(SERVICE_TYPES.iter().all(|t| kind_for_service(t).is_some()));
    }

    #[test]
    fn discovery_returning_nothing_is_a_normal_result_not_an_error() {
        // The static-IP farm this project started from must configure fine
        // with mDNS never succeeding, so `discover` returns a Vec, never a
        // Result — there is no failure mode a user needs to act on.
        let found = discover(Duration::from_millis(1));
        assert!(found.len() < 1000);
    }

    #[test]
    fn trailing_dots_are_trimmed_from_hostnames() {
        // mDNS hostnames are fully qualified with a trailing dot, which is
        // not what belongs in a `host` field being handed to a URL builder.
        assert_eq!(clean_host("voron.local."), "voron.local");
        assert_eq!(clean_host("voron.local"), "voron.local");
    }
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `source "$HOME/.cargo/env" && cargo test --manifest-path src-tauri/Cargo.toml discovery::`
Expected: FAIL — `cannot find function kind_for_service in this scope`.

- [ ] **Step 3: Implement discovery**

```rust
//! Bounded mDNS browsing for printers that advertise themselves.
//!
//! Discovery is an ACCELERATOR. Manual host/port entry is always available
//! and never gated behind it — a farm of printers at static IPs must be
//! fully configurable with mDNS never succeeding, which is why `discover`
//! returns a plain `Vec` and has no error path a user must act on.
//!
//! Results are returned from one bounded call rather than streamed as
//! events: the window is seconds, the result set is a handful of hosts, and
//! a returned vector needs no listener lifecycle and no cross-event
//! de-duplication.

use mdns_sd::{ServiceDaemon, ServiceEvent};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

pub const SERVICE_TYPES: &[&str] = &["_moonraker._tcp.local.", "_octoprint._tcp.local."];

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredPrinter {
    /// Matches `ConnectionConfig::kind`, so a suggestion can be applied
    /// directly without a lookup table in the UI.
    pub kind: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    /// Offered so a user on a network with broken `.local` resolution can
    /// still pick a working address.
    pub addresses: Vec<String>,
}

pub fn kind_for_service(service_type: &str) -> Option<&'static str> {
    match service_type {
        "_moonraker._tcp.local." => Some("moonraker"),
        "_octoprint._tcp.local." => Some("octoprint"),
        _ => None,
    }
}

pub fn clean_host(hostname: &str) -> String {
    hostname.trim_end_matches('.').to_string()
}

pub fn discover(window: Duration) -> Vec<DiscoveredPrinter> {
    let Ok(daemon) = ServiceDaemon::new() else {
        // No mDNS available on this host. Manual entry still works, so this
        // is an empty result, not an error.
        return Vec::new();
    };

    let mut receivers = Vec::new();
    for service_type in SERVICE_TYPES {
        if let Ok(rx) = daemon.browse(service_type) {
            receivers.push(rx);
        }
    }

    // Keyed so a printer advertising on both service types, or re-announcing
    // within the window, appears once.
    let mut found: BTreeMap<String, DiscoveredPrinter> = BTreeMap::new();
    let deadline = Instant::now() + window;

    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        let mut progressed = false;
        for rx in &receivers {
            // Poll each receiver briefly rather than blocking on one, so a
            // silent service type cannot consume the whole window.
            if let Ok(event) = rx.recv_timeout(remaining.min(Duration::from_millis(50))) {
                progressed = true;
                if let ServiceEvent::ServiceResolved(info) = event {
                    let Some(kind) = kind_for_service(info.get_type()) else { continue };
                    let host = clean_host(info.get_hostname());
                    let entry = DiscoveredPrinter {
                        kind: kind.to_string(),
                        name: info.get_fullname().split('.').next().unwrap_or(&host).to_string(),
                        host: host.clone(),
                        port: info.get_port(),
                        addresses: info.get_addresses().iter().map(|a| a.to_string()).collect(),
                    };
                    found.insert(format!("{host}:{}", entry.port), entry);
                }
            }
        }
        if !progressed && receivers.is_empty() {
            break;
        }
    }

    let _ = daemon.shutdown();
    found.into_values().collect()
}
```

Add to `src-tauri/src/connections/mod.rs`:

```rust
pub mod discovery;
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `source "$HOME/.cargo/env" && cargo test --manifest-path src-tauri/Cargo.toml discovery::`
Expected: 4 passed.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/connections/discovery.rs src-tauri/src/connections/mod.rs
git commit -m "feat: add bounded mDNS discovery for Moonraker and OctoPrint hosts"
```

---

### Task 7: Commands and app wiring

Thin `#[tauri::command]` wrappers plus the `setup()` change that starts supervisors for already-configured printers at launch.

**Files:**
- Create: `src-tauri/src/connections/commands.rs`
- Modify: `src-tauri/src/connections/mod.rs`, `src-tauri/src/lib.rs`

**Interfaces:**
- Produces commands: `set_printer_connection`, `clear_printer_connection`, `test_printer_connection`, `credential_store_info`, `discover_printers`, `printer_statuses`.

**No Tauri capability change is needed.** Verified against this repo's generated `acl-manifests.json`: `capabilities/default.json` already grants `core:default`, whose set includes `core:event:default` (the frontend's `listen`), and app-defined commands registered through `generate_handler!` are never gated by capability entries — as the phase-1 commands already demonstrate under this exact file.

- [ ] **Step 1: Write the failing test**

Command bodies are Tauri-bound, so what is tested is the one piece with real logic in it: deciding what to persist versus what to hand to the credential store.

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_submitted_api_key_never_lands_in_the_persisted_config() {
        let (config, secret) = split_submission(ConnectionSubmission {
            kind: MOONRAKER_KIND.to_string(),
            host: "voron.local".to_string(),
            port: 7125,
            use_tls: false,
            api_key: Some("s3cret".to_string()),
        }, "prn-1");

        assert_eq!(secret.as_deref(), Some("s3cret"));
        assert_eq!(config.credential_ref.as_deref(), Some("farm3d/printer/prn-1/apikey"));
        let json = serde_json::to_string(&config).unwrap();
        assert!(!json.contains("s3cret"), "the secret leaked into the persisted config");
    }

    #[test]
    fn an_empty_api_key_stores_no_reference_at_all() {
        // Trusted-LAN Moonraker instances need no key; a dangling
        // credentialRef pointing at nothing would make the Connection tab
        // claim a credential exists when none does.
        let (config, secret) = split_submission(ConnectionSubmission {
            kind: MOONRAKER_KIND.to_string(),
            host: "voron.local".to_string(),
            port: 7125,
            use_tls: false,
            api_key: Some("   ".to_string()),
        }, "prn-1");
        assert_eq!(secret, None);
        assert_eq!(config.credential_ref, None);
    }

    #[test]
    fn omitting_the_api_key_field_preserves_the_existing_reference() {
        // The UI never echoes a stored secret back, so "no api_key in this
        // submission" must mean "leave the stored one alone", not "clear it".
        let (config, secret) = split_submission(ConnectionSubmission {
            kind: MOONRAKER_KIND.to_string(),
            host: "voron.local".to_string(),
            port: 7125,
            use_tls: false,
            api_key: None,
        }, "prn-1");
        assert_eq!(secret, None);
        assert_eq!(config.credential_ref.as_deref(), Some("farm3d/printer/prn-1/apikey"));
    }
}
```

Note the deliberate asymmetry the last two tests pin down: `api_key: None` means "unchanged", `api_key: Some("")` means "cleared". Getting these the same way round would either strand secrets or wipe them on every unrelated edit.

- [ ] **Step 2: Run it to verify it fails**

Run: `source "$HOME/.cargo/env" && cargo test --manifest-path src-tauri/Cargo.toml commands::`
Expected: FAIL — `cannot find function split_submission in this scope`.

- [ ] **Step 3: Implement the commands**

```rust
//! The `#[tauri::command]` surface for connections. Thin by design — every
//! decision worth testing lives in a sibling module.

use super::credentials::{credential_ref_for, CredentialStore, CredentialStoreKind};
use super::discovery::{discover, DiscoveredPrinter};
use super::moonraker::MoonrakerConnection;
use super::supervisor::ConnectionManager;
use super::{ConnectionConfig, PrinterConnection, PrinterStatus, ProbeResult, MOONRAKER_KIND};
use crate::catalog::resolve::{resolve_printer, ResolvedPrinter};
use crate::catalog::Catalog;
use crate::printers::{load_printers_from, write_printers_to};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Manager};

const DISCOVERY_WINDOW: Duration = Duration::from_secs(3);

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionSubmission {
    pub kind: String,
    pub host: String,
    pub port: u16,
    #[serde(default)]
    pub use_tls: bool,
    /// `None` = leave the stored secret untouched (the UI never echoes it
    /// back). `Some("")` = clear it. These must stay distinct.
    #[serde(default)]
    pub api_key: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialStoreInfo {
    pub kind: CredentialStoreKind,
    /// Why the keychain was unavailable, so the Connection tab can explain
    /// the fallback rather than silently downgrading.
    pub reason: Option<String>,
}

/// Splits a submission into what gets persisted and what goes to the
/// credential store. The whole point is that the secret exits here and is
/// never part of the returned config.
pub fn split_submission(
    submission: ConnectionSubmission,
    printer_id: &str,
) -> (ConnectionConfig, Option<String>) {
    let key_ref = credential_ref_for(printer_id);
    let (credential_ref, secret) = match submission.api_key.as_deref() {
        None => (Some(key_ref), None),
        Some(key) if key.trim().is_empty() => (None, None),
        Some(key) => (Some(key_ref), Some(key.to_string())),
    };
    (
        ConnectionConfig {
            kind: submission.kind,
            host: submission.host,
            port: submission.port,
            use_tls: submission.use_tls,
            credential_ref,
        },
        secret,
    )
}

fn config_dir(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    app.path().app_config_dir().map_err(|e| e.to_string())
}

fn store(app: &AppHandle) -> Result<CredentialStore, String> {
    Ok(CredentialStore::detect(config_dir(app)?))
}

fn api_key_for(app: &AppHandle, config: &ConnectionConfig) -> Result<Option<String>, String> {
    match &config.credential_ref {
        None => Ok(None),
        Some(key) => store(app)?.get(key),
    }
}

fn adapter(
    config: &ConnectionConfig,
    api_key: Option<String>,
) -> Result<Box<dyn PrinterConnection>, String> {
    match config.kind.as_str() {
        MOONRAKER_KIND => Ok(Box::new(MoonrakerConnection::new(config.clone(), api_key))),
        other => Err(format!("This build cannot speak `{other}` connections")),
    }
}

#[tauri::command]
pub fn set_printer_connection(
    app: AppHandle,
    catalog: tauri::State<Arc<Catalog>>,
    manager: tauri::State<Arc<ConnectionManager>>,
    id: String,
    submission: ConnectionSubmission,
) -> Result<ResolvedPrinter, String> {
    let dir = config_dir(&app)?;
    let mut file = load_printers_from(&dir)?;
    let (config, secret) = split_submission(submission, &id);

    // Write the secret BEFORE persisting the reference: a config pointing at
    // a credential that was never stored is worse than a stored credential
    // nothing points at yet.
    if let (Some(secret), Some(key)) = (secret, config.credential_ref.as_deref()) {
        store(&app)?.set(key, &secret)?;
    }

    {
        let stored = file
            .printers
            .iter_mut()
            .find(|p| p.id == id)
            .ok_or_else(|| format!("no printer with id {id:?}"))?;
        stored.connection = Some(config.clone());
    }
    write_printers_to(&dir, &file)?;

    let api_key = api_key_for(&app, &config)?;
    manager.start(id.clone(), config, api_key);

    Ok(resolve_printer(&catalog, file.printers.iter().find(|p| p.id == id).unwrap()))
}

#[tauri::command]
pub fn clear_printer_connection(
    app: AppHandle,
    catalog: tauri::State<Arc<Catalog>>,
    manager: tauri::State<Arc<ConnectionManager>>,
    id: String,
) -> Result<ResolvedPrinter, String> {
    let dir = config_dir(&app)?;
    let mut file = load_printers_from(&dir)?;
    manager.stop(&id);

    {
        let stored = file
            .printers
            .iter_mut()
            .find(|p| p.id == id)
            .ok_or_else(|| format!("no printer with id {id:?}"))?;
        stored.connection = None;
    }
    write_printers_to(&dir, &file)?;
    // Removing the secret last: a failure here leaves an orphaned credential,
    // which is harmless, rather than an unusable config.
    store(&app)?.delete(&credential_ref_for(&id))?;

    Ok(resolve_printer(&catalog, file.printers.iter().find(|p| p.id == id).unwrap()))
}

/// Probes the SUBMITTED config, not the stored one, so "Test connection"
/// validates what the user typed before they commit it.
#[tauri::command]
pub async fn test_printer_connection(
    app: AppHandle,
    id: String,
    submission: ConnectionSubmission,
) -> Result<ProbeResult, String> {
    let (config, secret) = split_submission(submission, &id);
    let api_key = match secret {
        Some(secret) => Some(secret),
        None => api_key_for(&app, &config)?,
    };
    adapter(&config, api_key)?.probe().await.map_err(|e| e.to_string())
}

#[tauri::command]
pub fn credential_store_info(app: AppHandle) -> Result<CredentialStoreInfo, String> {
    let store = store(&app)?;
    Ok(CredentialStoreInfo {
        kind: store.kind(),
        reason: store.unavailable_reason().map(str::to_string),
    })
}

#[tauri::command]
pub async fn discover_printers() -> Vec<DiscoveredPrinter> {
    // mdns-sd's receiver is blocking, so this belongs on the blocking pool
    // rather than parked on an async worker for three seconds.
    tauri::async_runtime::spawn_blocking(|| discover(DISCOVERY_WINDOW))
        .await
        .unwrap_or_default()
}

#[tauri::command]
pub fn printer_statuses(
    manager: tauri::State<Arc<ConnectionManager>>,
) -> HashMap<String, PrinterStatus> {
    manager.statuses()
}
```

Add to `src-tauri/src/connections/mod.rs`:

```rust
pub mod commands;
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `source "$HOME/.cargo/env" && cargo test --manifest-path src-tauri/Cargo.toml commands::`
Expected: 3 passed.

- [ ] **Step 5: Wire the manager into `setup()` and register the commands**

In `src-tauri/src/lib.rs`, add the imports:

```rust
use connections::commands::{
    clear_printer_connection, credential_store_info, discover_printers, printer_statuses,
    set_printer_connection, test_printer_connection,
};
use connections::supervisor::ConnectionManager;
```

Inside `.setup(|app| { … })`, after `app.manage(Arc::new(snapshot));`:

```rust
            let manager = Arc::new(ConnectionManager::new(app.handle().clone()));
            // Reconnect everything that was already configured, so a printer
            // is live on the dashboard without the user opening its tab.
            if let Ok(dir) = app.path().app_config_dir() {
                if let Ok(file) = printers::load_printers_from(&dir) {
                    let store = connections::credentials::CredentialStore::detect(dir);
                    for stored in &file.printers {
                        let Some(config) = stored.connection.clone() else { continue };
                        let api_key = config
                            .credential_ref
                            .as_deref()
                            .and_then(|key| store.get(key).ok().flatten());
                        manager.start(stored.id.clone(), config, api_key);
                    }
                }
            }
            app.manage(manager);
```

`printers` must become `pub mod printers;` in `lib.rs` for `load_printers_from` to be reachable here.

Add to `tauri::generate_handler![…]`:

```rust
            set_printer_connection,
            clear_printer_connection,
            test_printer_connection,
            credential_store_info,
            discover_printers,
            printer_statuses,
```

- [ ] **Step 6: Verify the whole backend builds and passes**

Run: `source "$HOME/.cargo/env" && cargo test --manifest-path src-tauri/Cargo.toml`
Expected: all pass, no warnings about unused imports.

- [ ] **Step 7: Commit**

```bash
git add src-tauri/src/connections/commands.rs src-tauri/src/connections/mod.rs src-tauri/src/lib.rs
git commit -m "feat: add connection commands and start supervisors at launch"
```

---

### Task 8: Frontend types and store

Types the frontend's `connection` slot, adds connection mutations, and merges live status onto printer rows.

**Files:**
- Modify: `src/printers/types.ts`, `src/printers/printer-store.ts`, `src/printers/printer-store.test.ts`

**Interfaces:**
- Produces: `ConnectionConfig`, `ConnectionSubmission`, `ConnectionState`, `PrinterStatus`, `ProbeResult`, `DiscoveredPrinter`, `CredentialStoreInfo` types; `setConnection`, `clearConnection`, `testConnection`, `discoverPrinters`, `credentialStoreInfo`, `startStatusListener` from the store.

**Ruling on where live status lives:** it is merged onto each `ResolvedPrinter` as `runtimeStatus`, in `printer-store.ts`, rather than in a second store. A separate connection store would need to write into the printer list anyway, and two stores owning one list is worse than one file growing by ~90 lines.

- [ ] **Step 1: Add the types**

In `src/printers/types.ts`, replace `connection: unknown | null;` and `runtimeStatus?: unknown;` on `ResolvedPrinter` with `connection: ConnectionConfig | null;` and `runtimeStatus?: PrinterStatus;`, and append:

```ts
export interface ConnectionConfig {
  kind: string;
  host: string;
  port: number;
  useTls: boolean;
  /** A key into the credential store — never the secret itself. */
  credentialRef?: string;
}

/** What the Connection tab submits. `apiKey: undefined` leaves the stored
 *  secret untouched (the UI never echoes it back); `apiKey: ""` clears it. */
export interface ConnectionSubmission {
  kind: string;
  host: string;
  port: number;
  useTls: boolean;
  apiKey?: string;
}

export type ConnectionState = "connecting" | "online" | "offline" | "error";

export interface PrinterStatus {
  connectionState: ConnectionState;
  error?: string;
  jobState?: string;
  jobName?: string;
  /** 0..1 */
  progress?: number;
  nozzleTempC?: number;
  nozzleTargetC?: number;
  bedTempC?: number;
  bedTargetC?: number;
  printDurationS?: number;
  updatedAt: string;
}

export interface ReportedCapabilities {
  bedWidthMm?: number;
  bedDepthMm?: number;
  printableHeightMm?: number;
}

export interface ProbeResult {
  kind: string;
  hostSoftware: string;
  firmware: string;
  reportedName: string;
  state: string;
  stateMessage: string;
  reported: ReportedCapabilities;
}

export interface DiscoveredPrinter {
  kind: string;
  name: string;
  host: string;
  port: number;
  addresses: string[];
}

export interface CredentialStoreInfo {
  kind: "keychain" | "file";
  reason?: string;
}
```

- [ ] **Step 2: Write the failing store tests**

Append to `src/printers/printer-store.test.ts`, following its existing `vi.hoisted` + `vi.resetModules()` idiom (mandatory — the store is a module-level singleton):

```ts
  it("merges a status event onto the matching printer and leaves siblings alone", async () => {
    const store = await import("./printer-store");
    await store.loadPrinters();
    const [first, second] = store.printers();

    store.applyStatus(first.id, {
      connectionState: "online",
      nozzleTempC: 201.4,
      updatedAt: "2026-08-20T14:02:11Z",
    });

    expect(store.printers()[0].runtimeStatus?.nozzleTempC).toBe(201.4);
    expect(store.printers().find((p) => p.id === second.id)?.runtimeStatus).toBeUndefined();
  });

  it("ignores a status event for a printer it does not know", async () => {
    // A stale event can arrive after a delete; it must not resurrect a row.
    const store = await import("./printer-store");
    await store.loadPrinters();
    const before = store.printers().length;
    store.applyStatus("prn-ghost", {
      connectionState: "online",
      updatedAt: "2026-08-20T14:02:11Z",
    });
    expect(store.printers()).toHaveLength(before);
  });

  it("does not invoke connection mutations in the web fallback", async () => {
    const store = await import("./printer-store");
    await store.loadPrinters();
    await store.setConnection("prn-voron-1", {
      kind: "moonraker",
      host: "voron.local",
      port: 7125,
      useTls: false,
    });
    expect(invoke).not.toHaveBeenCalled();
  });
```

- [ ] **Step 3: Run them to verify they fail**

Run: `npx vitest run src/printers/printer-store.test.ts`
Expected: FAIL — `store.applyStatus is not a function`.

- [ ] **Step 4: Implement the store additions**

Append to `src/printers/printer-store.ts` (and extend its type import list with the new names):

```ts
/** Merges live status onto one printer row. Unknown ids are ignored — a
 *  status event can arrive after a delete, and must not resurrect the row. */
export function applyStatus(id: string, status: PrinterStatus): void {
  if (!state.printers.some((p) => p.id === id)) return;
  setState("printers", (p) => p.id === id, "runtimeStatus", status);
}

/** Subscribes to the supervisor's status events, then backfills whatever it
 *  already knows — a printer that came online before this listener attached
 *  would otherwise show nothing until its next change. Returns an unlisten fn. */
export async function startStatusListener(): Promise<() => void> {
  if (!isTauri()) return () => {};
  const { listen } = await import("@tauri-apps/api/event");
  const unlisten = await listen<{ id: string; status: PrinterStatus }>(
    "printer-status",
    (event) => applyStatus(event.payload.id, event.payload.status),
  );
  try {
    const known = await invoke<Record<string, PrinterStatus>>("printer_statuses");
    for (const [id, status] of Object.entries(known)) applyStatus(id, status);
  } catch (e) {
    reportError(e);
  }
  return unlisten;
}

export async function setConnection(id: string, submission: ConnectionSubmission): Promise<void> {
  if (!isTauri()) return;
  try {
    spliceResolved(await invoke<ResolvedPrinter>("set_printer_connection", { id, submission }));
  } catch (e) {
    reportError(e);
  }
}

export async function clearConnection(id: string): Promise<void> {
  if (!isTauri()) return;
  try {
    spliceResolved(await invoke<ResolvedPrinter>("clear_printer_connection", { id }));
  } catch (e) {
    reportError(e);
  }
}

/** Rejects rather than reporting into the banner: the Connection tab renders
 *  a probe failure inline, next to the fields the user needs to correct. */
export async function testConnection(
  id: string,
  submission: ConnectionSubmission,
): Promise<ProbeResult> {
  if (!isTauri()) throw new Error("Testing a connection needs the desktop app");
  return invoke<ProbeResult>("test_printer_connection", { id, submission });
}

export async function discoverPrinters(): Promise<DiscoveredPrinter[]> {
  if (!isTauri()) return [];
  try {
    return await invoke<DiscoveredPrinter[]>("discover_printers");
  } catch (e) {
    reportError(e);
    return [];
  }
}

export async function credentialStoreInfo(): Promise<CredentialStoreInfo | null> {
  if (!isTauri()) return null;
  try {
    return await invoke<CredentialStoreInfo>("credential_store_info");
  } catch (e) {
    reportError(e);
    return null;
  }
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `npx vitest run src/printers/printer-store.test.ts`
Expected: all pass.

- [ ] **Step 6: Commit**

```bash
git add src/printers/types.ts src/printers/printer-store.ts src/printers/printer-store.test.ts
git commit -m "feat: type the connection slot and add connection actions to the printer store"
```

---

### Task 9: The Connection tab

Replaces phase 1's "Configured in a later phase." placeholder.

**Files:**
- Create: `src/screens/PrinterConnectionPanel.tsx`, `.module.css`, `.test.tsx`

**Interfaces:**
- Consumes: `ResolvedPrinter`, the connection types and store actions from Task 8, and `Button`/`TextField`/`Select`/`Field`/`Chip` from the design system.
- Produces: `PrinterConnectionPanel({ printer })`, and the pure `buildMismatches(profile, reported) -> string[]`.

- [ ] **Step 1: Write the failing tests**

```tsx
import { fireEvent, render, screen } from "@solidjs/testing-library";
import { describe, expect, it, vi } from "vitest";
import { buildMismatches, PrinterConnectionPanel } from "./PrinterConnectionPanel";

const setConnection = vi.hoisted(() => vi.fn().mockResolvedValue(undefined));
const testConnection = vi.hoisted(() => vi.fn());
vi.mock("../printers/printer-store", () => ({
  setConnection,
  clearConnection: vi.fn(),
  testConnection,
  discoverPrinters: vi.fn().mockResolvedValue([]),
  credentialStoreInfo: vi.fn().mockResolvedValue({ kind: "keychain" }),
}));

const PROFILE = {
  bedShape: { kind: "rectangular", widthMm: 256, depthMm: 256, originXMm: 0, originYMm: 0 },
  printableHeightMm: 256,
} as never;

describe("buildMismatches", () => {
  it("is silent when the host agrees with the catalog", () => {
    expect(
      buildMismatches(PROFILE, { bedWidthMm: 256, bedDepthMm: 256, printableHeightMm: 256 }),
    ).toEqual([]);
  });

  it("reports a build volume the host disagrees with", () => {
    // A mismatch usually means the wrong catalog variant was picked when the
    // printer was added — worth surfacing, not hiding.
    const mismatches = buildMismatches(PROFILE, { bedWidthMm: 220 });
    expect(mismatches).toHaveLength(1);
    expect(mismatches[0]).toMatch(/220/);
  });

  it("says nothing about values the host did not report", () => {
    // An unhomed Klipper reports no axis limits. Absence is not disagreement.
    expect(buildMismatches(PROFILE, {})).toEqual([]);
  });

  it("tolerates a millimetre of float noise", () => {
    expect(buildMismatches(PROFILE, { bedWidthMm: 256.4 })).toEqual([]);
  });

  it("says nothing for a non-rectangular bed", () => {
    const polygon = { bedShape: { kind: "polygon", points: [] }, printableHeightMm: 256 } as never;
    expect(buildMismatches(polygon, { bedWidthMm: 1 })).toEqual([]);
  });
});

describe("PrinterConnectionPanel", () => {
  const printer = {
    id: "prn-1",
    name: "Bay 1",
    profile: PROFILE,
    connection: null,
  } as never;

  it("defaults the kind from the catalog's suggestedHostType", async () => {
    const suggested = {
      ...printer,
      profile: { ...PROFILE, suggestedHostType: "moonraker" },
    } as never;
    render(() => <PrinterConnectionPanel printer={suggested} />);
    expect(await screen.findByRole("button", { name: /Moonraker/ })).toBeInTheDocument();
  });

  it("submits host and port without echoing an unset API key", async () => {
    render(() => <PrinterConnectionPanel printer={printer} />);
    fireEvent.input(screen.getByLabelText("Host"), { target: { value: "voron.local" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    expect(setConnection).toHaveBeenCalledWith(
      "prn-1",
      expect.objectContaining({ host: "voron.local", port: 7125 }),
    );
  });

  it("renders a probe failure inline rather than throwing it away", async () => {
    testConnection.mockRejectedValueOnce("Could not reach the printer: refused");
    render(() => <PrinterConnectionPanel printer={printer} />);
    fireEvent.click(screen.getByRole("button", { name: "Test connection" }));
    expect(await screen.findByText(/Could not reach the printer/)).toBeInTheDocument();
  });

  it("shows which credential store is live", async () => {
    render(() => <PrinterConnectionPanel printer={printer} />);
    expect(await screen.findByText(/OS keychain/)).toBeInTheDocument();
  });
});
```

- [ ] **Step 2: Run them to verify they fail**

Run: `npx vitest run src/screens/PrinterConnectionPanel.test.tsx`
Expected: FAIL — cannot resolve `./PrinterConnectionPanel`.

- [ ] **Step 3: Implement the panel**

```tsx
import { createMemo, createResource, createSignal, For, Show } from "solid-js";
import { Button, Chip, Field, Select, TextField } from "../design-system";
import {
  clearConnection,
  credentialStoreInfo,
  discoverPrinters,
  setConnection,
  testConnection,
} from "../printers/printer-store";
import type {
  ConnectionSubmission,
  PrinterProfile,
  ProbeResult,
  ReportedCapabilities,
  ResolvedPrinter,
} from "../printers/types";
import styles from "./PrinterConnectionPanel.module.css";

/** Only what this build can actually speak. Phase 3 appends OctoPrint and
 *  ElegooLink here as each adapter lands — listing them now as disabled
 *  entries would need a prop `Select` does not have (verified: `SelectProps`
 *  exposes no `optionDisabled`), and offering a kind that errors on save is
 *  worse than not offering it. */
const KINDS = [{ value: "moonraker", label: "Moonraker (Klipper)" }];

const DEFAULT_PORTS: Record<string, number> = { moonraker: 7125, octoprint: 80 };

/** Millimetre-scale float noise is not a disagreement worth a warning. */
const TOLERANCE_MM = 1;

/**
 * Compares what the host reports against the catalog Profile. A mismatch
 * usually means the wrong catalog variant was picked in the add-printer
 * flow — worth surfacing, since phase 3 will slice against these numbers.
 *
 * Absence is never a mismatch: an unhomed or shut-down Klipper reports no
 * axis limits at all, and warning about that would train users to ignore
 * this box.
 */
export function buildMismatches(
  profile: PrinterProfile,
  reported: ReportedCapabilities,
): string[] {
  const mismatches: string[] = [];
  const check = (label: string, catalog: number | undefined, host: number | undefined) => {
    if (catalog === undefined || host === undefined) return;
    if (Math.abs(catalog - host) <= TOLERANCE_MM) return;
    mismatches.push(`${label}: catalog says ${catalog} mm, the printer reports ${host} mm`);
  };

  // Only a rectangular bed has a width/depth to compare; a polygon bed's
  // extents are not what axis limits describe.
  if (profile.bedShape.kind === "rectangular") {
    check("Bed width", profile.bedShape.widthMm, reported.bedWidthMm);
    check("Bed depth", profile.bedShape.depthMm, reported.bedDepthMm);
  }
  check("Printable height", profile.printableHeightMm, reported.printableHeightMm);
  return mismatches;
}

export interface PrinterConnectionPanelProps {
  printer: ResolvedPrinter;
}

export function PrinterConnectionPanel(props: PrinterConnectionPanelProps) {
  const existing = () => props.printer.connection;
  const [kind, setKind] = createSignal(
    existing()?.kind ?? props.printer.profile.suggestedHostType ?? "moonraker",
  );
  const [host, setHost] = createSignal(existing()?.host ?? "");
  const [port, setPort] = createSignal(String(existing()?.port ?? DEFAULT_PORTS[kind()] ?? 7125));
  // Never seeded from storage — a stored secret is never echoed back. Empty
  // therefore means "leave it alone", which is why the submission below sends
  // `undefined` rather than `""`.
  const [apiKey, setApiKey] = createSignal("");
  const [probe, setProbe] = createSignal<ProbeResult | null>(null);
  const [probeError, setProbeError] = createSignal<string | null>(null);
  const [testing, setTesting] = createSignal(false);

  const [store] = createResource(credentialStoreInfo);
  const [discovered, { refetch: rediscover }] = createResource(discoverPrinters);

  const submission = (): ConnectionSubmission => ({
    kind: kind(),
    host: host().trim(),
    port: Number(port()) || DEFAULT_PORTS[kind()] || 7125,
    useTls: existing()?.useTls ?? false,
    apiKey: apiKey() === "" ? undefined : apiKey(),
  });

  const mismatches = createMemo(() => {
    const result = probe();
    return result ? buildMismatches(props.printer.profile, result.reported) : [];
  });

  async function onTest() {
    setTesting(true);
    setProbeError(null);
    try {
      setProbe(await testConnection(props.printer.id, submission()));
    } catch (e) {
      setProbe(null);
      setProbeError(String(e));
    } finally {
      setTesting(false);
    }
  }

  return (
    <div class={styles.panel}>
      <Field label="Kind">
        <Select
          label="Kind"
          options={KINDS}
          optionValue={(k: (typeof KINDS)[number]) => k.value}
          optionLabel={(k: (typeof KINDS)[number]) => k.label}
          value={KINDS.find((k) => k.value === kind())}
          onChange={(k: (typeof KINDS)[number]) => {
            setKind(k.value);
            setPort(String(DEFAULT_PORTS[k.value] ?? 7125));
          }}
        />
      </Field>

      <TextField label="Host" value={host()} onChange={setHost} placeholder="voron.local" />
      <TextField label="Port" value={port()} onChange={setPort} />
      <TextField
        label="API key"
        type="password"
        value={apiKey()}
        onChange={setApiKey}
        placeholder={existing()?.credentialRef ? "Stored — leave blank to keep" : "Optional"}
      />

      <Show when={store()}>
        {(info) => (
          <p class={styles.note}>
            Credentials stored in:{" "}
            {info().kind === "keychain" ? "OS keychain" : "credentials.json"}
            <Show when={info().reason}>
              {(reason) => <span class={styles.warn}> — no OS keychain available ({reason()})</span>}
            </Show>
          </p>
        )}
      </Show>

      {/* Discovery is an accelerator: the fields above work with it empty. */}
      <div class={styles.discovery}>
        <div class={styles.discoveryHeader}>
          <span class={styles.sectionTitle}>Discovered on this network</span>
          <Button variant="ghost" onClick={() => void rediscover()}>
            Rescan
          </Button>
        </div>
        <Show
          when={(discovered() ?? []).length > 0}
          fallback={<p class={styles.note}>Nothing found — enter the host above.</p>}
        >
          <For each={discovered()}>
            {(found) => (
              <Button
                variant="ghost"
                onClick={() => {
                  setKind(found.kind);
                  setHost(found.host);
                  setPort(String(found.port));
                }}
              >
                {found.name} — {found.host}:{found.port}
              </Button>
            )}
          </For>
        </Show>
      </div>

      <div class={styles.actions}>
        <Button variant="secondary" disabled={testing()} onClick={() => void onTest()}>
          {testing() ? "Testing…" : "Test connection"}
        </Button>
        <Button variant="primary" onClick={() => void setConnection(props.printer.id, submission())}>
          Save
        </Button>
        <Show when={existing()}>
          <Button variant="danger" onClick={() => void clearConnection(props.printer.id)}>
            Disconnect
          </Button>
        </Show>
      </div>

      <Show when={probeError()}>
        {(message) => <p class={styles.error}>{message()}</p>}
      </Show>

      <Show when={probe()}>
        {(result) => (
          <div class={styles.probe}>
            <Chip>{result().state}</Chip>
            <p class={styles.note}>
              {result().reportedName} — {result().hostSoftware} / {result().firmware}
            </p>
            <Show when={result().stateMessage}>
              {(message) => <p class={styles.note}>{message()}</p>}
            </Show>
            <Show when={mismatches().length > 0}>
              <ul class={styles.mismatches}>
                <For each={mismatches()}>{(text) => <li>{text}</li>}</For>
              </ul>
              <p class={styles.note}>
                This usually means a different catalog variant matches this machine.
              </p>
            </Show>
          </div>
        )}
      </Show>
    </div>
  );
}
```

- [ ] **Step 4: Write the stylesheet**

Every value below references a design token — no literal colors, sizes, or radii (see Global Constraints). Create `src/screens/PrinterConnectionPanel.module.css`:

```css
.panel {
  display: flex;
  flex-direction: column;
  gap: 0.5rem;
}

.sectionTitle {
  font-family: var(--f3d-type-label-font);
  font-size: var(--f3d-type-label-size);
  font-weight: var(--f3d-type-label-weight);
  letter-spacing: var(--f3d-type-label-tracking);
  color: var(--f3d-color-text-muted);
}

.note {
  font-size: var(--f3d-type-body-size);
  color: var(--f3d-color-text-muted);
}

.warn {
  color: var(--f3d-color-warning);
}

.error {
  font-size: var(--f3d-type-body-size);
  color: var(--f3d-color-danger);
}

.discovery,
.probe {
  display: flex;
  flex-direction: column;
  gap: 0.25rem;
  padding: 0.5rem;
  border: 1px solid var(--f3d-color-border);
  border-radius: var(--f3d-radius-sm);
  background-color: var(--f3d-color-surface-raised);
}

.discoveryHeader {
  display: flex;
  align-items: center;
  justify-content: space-between;
}

.actions {
  display: flex;
  gap: 0.375rem;
}

.mismatches {
  margin: 0;
  padding-left: 1rem;
  font-size: var(--f3d-type-body-size);
  color: var(--f3d-color-warning);
}
```

Every token above is verified to exist. Note that the `--f3d-color-*` custom properties are **not declared in any stylesheet** — `theme-engine.ts` sets them on `:root` at runtime from the theme objects in `src/design-system/themes/`, kebab-casing each role name (`onWarning` → `--f3d-color-on-warning`). Grepping the CSS for a token definition will find nothing; `src/design-system/tokens/types.ts` is the authoritative list of roles.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `npx vitest run src/screens/PrinterConnectionPanel.test.tsx`
Expected: all pass.

The "defaults the kind from suggestedHostType" test asserts on the `Select` trigger's accessible name. Kobalte composes that name from the label **and** the current value (`"Kind Moonraker (Klipper)"` once a value is selected, not just `"Kind"`), so the regex query in the test is deliberate — an exact-match `{ name: "Kind" }` will not find it.

- [ ] **Step 6: Commit**

```bash
git add src/screens/PrinterConnectionPanel.tsx src/screens/PrinterConnectionPanel.module.css src/screens/PrinterConnectionPanel.test.tsx
git commit -m "feat: add the printer connection tab"
```

---

### Task 10: Live status on the dashboard

Mounts the panel, puts connection state on the cards, and finally lets `summarizePrinters` count something.

**Files:**
- Modify: `src/screens/PrinterDashboard.tsx`, `.module.css`, `.test.tsx`, `src/App.tsx`

- [ ] **Step 1: Write the failing tests**

Append to `src/screens/PrinterDashboard.test.tsx`:

```tsx
  it("counts connection states once printers report them", () => {
    // Phase 1 could only say "3 printers" — there was nothing to count.
    const printers = [
      { ...PRINTER, id: "a", runtimeStatus: { connectionState: "online", updatedAt: "" } },
      { ...PRINTER, id: "b", runtimeStatus: { connectionState: "offline", updatedAt: "" } },
      { ...PRINTER, id: "c" },
    ] as never[];
    expect(summarizePrinters(printers)).toBe("3 printers — 1 online, 1 offline");
  });

  it("still says only the count when nothing has reported", () => {
    expect(summarizePrinters([PRINTER] as never[])).toBe("1 printer");
  });

  it("badges a printer with its connection state", () => {
    const printers = [
      { ...PRINTER, id: "a", runtimeStatus: { connectionState: "online", updatedAt: "" } },
    ] as never[];
    render(() => <PrinterDashboard printers={printers} />);
    expect(screen.getByText("online")).toBeInTheDocument();
  });

  it("renders an unreported temperature as a dash, never as zero", () => {
    const printers = [
      { ...PRINTER, id: "a", runtimeStatus: { connectionState: "online", updatedAt: "" } },
    ] as never[];
    render(() => <PrinterDashboard printers={printers} />);
    expect(screen.queryByText(/0 °C/)).not.toBeInTheDocument();
  });
```

- [ ] **Step 2: Run them to verify they fail**

Run: `npx vitest run src/screens/PrinterDashboard.test.tsx`
Expected: FAIL on the summary assertion — it currently returns `"3 printers"`.

- [ ] **Step 3: Extend `summarizePrinters`**

Replace it in `src/screens/PrinterDashboard.tsx`:

```tsx
/** "3 printers — 1 online, 1 offline" for AppShell's status bar. Printers
 *  that have never reported are counted in the total only: a printer with no
 *  Connection configured is not "offline", it is simply not connected. */
export function summarizePrinters(printers: ResolvedPrinter[]): string {
  if (printers.length === 0) return "No printers";
  const total = `${printers.length} printer${printers.length === 1 ? "" : "s"}`;
  const counts = { online: 0, offline: 0, error: 0, connecting: 0 };
  for (const printer of printers) {
    const state = printer.runtimeStatus?.connectionState;
    if (state) counts[state] += 1;
  }
  const parts = (["online", "connecting", "offline", "error"] as const)
    .filter((state) => counts[state] > 0)
    .map((state) => `${counts[state]} ${state}`);
  return parts.length > 0 ? `${total} — ${parts.join(", ")}` : total;
}
```

- [ ] **Step 4: Add the status badge and readings to the card**

Inside `.badgeRow`, before the `Unlinked` badge:

```tsx
                          <Show when={printer.runtimeStatus}>
                            {(status) => (
                              <span
                                class={[styles.badge, styles[`state_${status().connectionState}`]].join(" ")}
                                title={status().error ?? `Updated ${status().updatedAt}`}
                              >
                                {status().connectionState}
                              </span>
                            )}
                          </Show>
```

And replace `.cardFooter`'s single span with:

```tsx
                        <div class={styles.cardFooter}>
                          <span>{printer.variantLabel}</span>
                          <Show when={printer.runtimeStatus}>
                            {(status) => (
                              // An unreported reading is an em dash, never a
                              // zero — "0 °C" reads as a real measurement.
                              <span class={styles.readings}>
                                {formatTemp(status().nozzleTempC)} / {formatTemp(status().bedTempC)}
                              </span>
                            )}
                          </Show>
                        </div>
```

With this helper beside `summarizePrinters`:

```tsx
function formatTemp(value: number | undefined): string {
  return value === undefined ? "—" : `${Math.round(value)} °C`;
}
```

- [ ] **Step 5: Mount the Connection tab**

Replace the placeholder tab content:

```tsx
                    {
                      value: "connection",
                      label: "Connection",
                      content: <PrinterConnectionPanel printer={printer()} />,
                    },
```

and add the import. Add the four state classes to `PrinterDashboard.module.css`, reusing the existing badge colors rather than introducing new ones:

```css
.state_online {
  composes: badgeAccent;
}

.state_connecting {
  composes: badgeMuted;
}

.state_offline {
  composes: badgeMuted;
}

.state_error {
  composes: badgeWarning;
}
```

- [ ] **Step 6: Start the status listener**

In `src/App.tsx`'s `onMount`, beside the existing `void loadPrinters();`:

```tsx
    let unlisten: (() => void) | undefined;
    void startStatusListener().then((fn) => (unlisten = fn));
    onCleanup(() => unlisten?.());
```

- [ ] **Step 7: Run the full suite**

Run: `just build && just test && source "$HOME/.cargo/env" && just test-rust`
Expected: all three pass.

- [ ] **Step 8: Commit**

```bash
git add src/screens/PrinterDashboard.tsx src/screens/PrinterDashboard.module.css src/screens/PrinterDashboard.test.tsx src/App.tsx
git commit -m "feat: show live connection state and temperatures on printer cards"
```

---

## End-to-end verification

Automated tests cannot cover the parts that need a real printer. Per `AGENTS.md`, a passing suite does not mean the UI renders correctly — run this before considering the phase done.

**Against a real Moonraker instance:**

1. `just dev`. Add a Klipper printer via the phase-1 add flow, select it, open **Connection**.
2. Confirm **Discovered on this network** lists the instance. If it does not, continue with manual entry — discovery is an accelerator and a farm at static IPs must configure fully without it.
3. Enter host and port, click **Test connection**. Confirm the probe renders Moonraker's version, Klipper's version, and the reported hostname.
4. Confirm the build-volume cross-check: it should be silent for a correctly-matched printer. Deliberately add a second printer bound to a *different* catalog variant, point it at the same host, and confirm the mismatch list appears.
5. **Save.** Confirm the card badge goes `connecting` → `online` and that temperatures appear and tick.
6. **The partial-update check — the one thing worth doing by hand.** Heat the nozzle only (`M104 S200`). Confirm the bed temperature on the card keeps its value rather than blanking. This is the failure mode Task 3 exists to prevent, and it only shows against a live stream.
7. Pull the printer's network cable. Confirm the badge goes to `offline`/`error` and that reconnect attempts space out rather than hammering. Plug back in; confirm it recovers without restarting farm3d.
8. Inspect `~/.config/farm3d/printers.json`: it must contain `credentialRef` and **must not contain the API key**. Confirm the key is in the OS keychain (`secret-tool search service farm3d` on Linux) — or, on a box with no Secret Service, in `credentials.json` with mode `0600`, with the Connection tab saying so.
9. Restart farm3d. Confirm the printer reconnects on launch without opening its Connection tab.

**Without a printer** (still worth doing): point a connection at a closed port and confirm the error says *reachability*, not credentials; then at a host running some other HTTP service and confirm the error is distinguishable.

---

## Notes for the phase-3 planner

Record these against `docs/superpowers/specs/2026-08-20-printer-adapters-design.md` as this phase lands:

- Whether `subscribe`'s channel signature survived OctoPrint's internal polling loop unchanged. The trait was shaped so it should; phase 3 is where that gets tested, and reshaping it is that phase's job, not evidence phase 2 was wrong.
- `reqwest` arrives with OctoPrint, not before.
- The `kind` field is deliberately a string, not an enum, so a phase-3 config round-trips through a phase-2 build instead of failing the whole `printers.json` load. `supervisor::build` and `commands::adapter` are the two places a new kind must be registered.
- ElegooLink still needs its protocol spike before any plan is written for it.

---

## Self-review

Checked against the spec after writing:

**Spec coverage.** Every settled decision maps to a task: dependency step (1), keychain-plus-specified-fallback with the store surfaced in the UI (2, 9), the `PrinterConnection` trait with probe and subscribe (1, 4), supervisor-owned reconnect (5), mDNS alongside always-available manual entry (6, 9), the Connection tab with a kind selector defaulting from `suggestedHostType` (9), live status on the existing card fields plus connection state on the badge (10). The wire contract added to the spec is implemented in Tasks 1 and 3.

**Three places this plan deliberately revises the spec**, each recorded in the spec itself as well: `reqwest` deferred to phase 3 (Moonraker's WebSocket serves the probe); discovery returns a `Vec` rather than streaming events; no Tauri capability change is needed, which corrects the spec's original guess.

**Type consistency.** `ProbeResult.reported` is `ReportedCapabilities` in Rust (Task 1), TypeScript (Task 8), and both test suites. `PrinterStatus` field names match across `mod.rs`, `protocol.rs`'s `to_status`, `types.ts`, and the dashboard. `credential_ref_for` produces the one string format written to `printers.json`, asserted in Tasks 2 and 7.

**One risk this plan does not eliminate.** Nothing here has spoken to a live Moonraker instance — the contract is documentation-derived, which is why every `PrinterStatus` field is optional, why unknown printer objects and unrecognized frames are ignored rather than fatal, and why the end-to-end checklist above is not optional. If step 6 of that checklist fails, the bug is in `StatusSnapshot::merge` and Task 3's test suite is where to reproduce it.

