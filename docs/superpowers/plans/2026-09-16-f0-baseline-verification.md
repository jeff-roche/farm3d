# F0 Baseline Verification Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> `superpowers:subagent-driven-development` (recommended) or
> `superpowers:executing-plans` to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Freeze and document the current farm3d persistence, command/event,
restart, automated-test, packaging, platform, and live-hardware baseline so later
v1 migrations have verified evidence rather than historical assumptions.

**Architecture:** F0 adds executable compatibility fixtures at the existing Rust
storage seams, a narrow mock-runtime Tauri IPC tracer, and two evidence documents
under `docs/superpowers/baselines/`. The tracer makes only the runtime generics
and startup-restoration orchestration needed to exercise production commands
through Tauri; the report distinguishes source trace, IPC/automated evidence,
packaged-app evidence, and unavailable live-hardware evidence.

**Tech Stack:** Rust 2021, Serde/serde_json, Cargo tests, Tauri 2, SolidJS,
TypeScript, Vitest, `just`, and Markdown evidence records.

**Spec:**
`docs/superpowers/plans/2026-09-16-complete-v1-implementation-approach.md`
(Phase F0, lines 261-315)

## Global Constraints

- Use baseline commit `e925d862f73416dac4036394e430fc9cf4bbe812` plus the
  approved v1 design/domain documents already present in the working tree.
- Do not change production behavior unless a verification step exposes a
  reproducible defect; if one appears, first add the smallest failing regression
  test, then make the smallest fix and record it in the baseline report.
- Do not modify unrelated `.devcontainer/` content.
- Keep credentials out of `printers.json`, frontend state, events, logs, and
  baseline documents. The committed credential fixture uses only the sentinel
  `F0_FIXTURE_SENTINEL`, never a real secret.
- Rust-related commands in non-interactive shells must begin with
  `source "$HOME/.cargo/env" &&`.
- Linux x86_64 is the intended initial release-support target, but its matrix
  row becomes `Supported` only after both package and executable-launch checks
  pass. If either fails, it remains `Candidate, unverified`, F0 remains open,
  and the failure is fixed or an available local display/tooling blocker is
  resolved before the PR is opened. Windows x86_64/aarch64 and macOS
  x86_64/arm64 remain candidate, unverified platforms.
- A broad Tauri bundle target is not platform evidence. Each supported-platform
  row must name its build, installed-or-packaged launch, and hardware-dependent
  evidence requirements.
- A live Moonraker claim requires a reachable representative instance. The F0
  planning environment has no `FARM3D_F0_MOONRAKER_URL`; execution must confirm
  it remains absent and record `Not run: no reachable instance available`
  rather than substituting protocol fixtures or asking the user. Live probing
  is deferred to A0.1, whose focused plan must provide the bounded,
  credential-safe adapter harness and controlled disconnect/reconnect procedure.
- F0 introduces no frontend behavior; nevertheless, the full repository gates
  are `just build`, `just test`, and
  `source "$HOME/.cargo/env" && just test-rust`.

## File Responsibility Map

- `src-tauri/tests/fixtures/persistence/v1/settings.json`: canonical current
  settings document consumed by compatibility tests and future migrations.
- `src-tauri/tests/fixtures/persistence/v1/printers.json`: canonical current
  schema-version-1 Printer document containing one Profile-only Printer and one
  connected Printer, a last-known-good Profile, an override, and a credential
  reference but no credential value.
- `src-tauri/tests/fixtures/persistence/v1/credentials.json`: canonical current
  file-backed credential map containing only the non-secret fixture sentinel.
- `src-tauri/src/settings.rs`: proves the baseline settings fixture loads through
  the real storage seam.
- `src-tauri/src/printers.rs`: proves the baseline Printer fixture loads through
  the real storage seam, preserves compatibility data, and does not contain the
  credential sentinel.
- `src-tauri/src/connections/credentials.rs`: proves the baseline file-backed
  credential fixture loads through the real credential seam.
- `src-tauri/src/connections/supervisor.rs`: makes `ConnectionManager` generic
  over `tauri::Runtime` so the same manager can run under production Wry and
  Tauri's mock runtime.
- `src-tauri/src/connections/commands.rs`: makes the four tracer-facing command
  signatures generic over the runtime without changing their payloads.
- `src-tauri/src/printers.rs`: also makes the create/list command signatures
  generic over the runtime without changing their payloads.
- `src-tauri/src/lib.rs`: extracts generic builder/startup restoration helpers;
  production `run()` and the IPC tracer execute the same startup path.
- `src-tauri/tests/f0_tauri_path.rs`: invokes create, Connection save, restart
  restoration, and status backfill through Tauri's mock IPC runtime.
- `docs/superpowers/baselines/2026-09-16-f0-baseline.md`: source inventory,
  command/event trace, persistence behavior, test evidence, package/launch
  evidence, and live-Moonraker evidence or explicit absence.
- `docs/superpowers/baselines/2026-09-16-supported-platforms.md`: release-support
  decision and evidence matrix.

---

### Task 0: Preserve the Approved Planning Inputs

**Files:**
- Modify: `CONTEXT.md`
- Create: `docs/superpowers/specs/2026-09-16-complete-v1-ui-workflows-design.md`
- Create: `docs/superpowers/plans/2026-09-16-complete-v1-implementation-approach.md`
- Create: `docs/superpowers/plans/2026-09-16-f0-baseline-verification.md`

**Interfaces:**
- Consumes: the approved working-tree design, domain vocabulary, umbrella
  approach, and independently approved F0 plan.
- Produces: committed inputs on the F0 branch so implementation reviewers and
  the PR can evaluate F0 against the exact approved documents.

- [ ] **Step 1: Confirm the approved files and exclude unrelated content**

Run `git status --short`, inspect all four approved files, and confirm
`.devcontainer/` remains untracked and unstaged. Do not stage any other path.

- [ ] **Step 2: Commit the approved inputs exactly**

```sh
git add CONTEXT.md \
  docs/superpowers/specs/2026-09-16-complete-v1-ui-workflows-design.md \
  docs/superpowers/plans/2026-09-16-complete-v1-implementation-approach.md \
  docs/superpowers/plans/2026-09-16-f0-baseline-verification.md
git commit -m "docs: define complete v1 delivery approach"
```

Expected: the commit contains exactly these four paths; `.devcontainer/` remains
untracked and untouched.

### Task 1: Freeze Current Persistence Formats

**Files:**
- Create: `src-tauri/tests/fixtures/persistence/v1/settings.json`
- Create: `src-tauri/tests/fixtures/persistence/v1/printers.json`
- Create: `src-tauri/tests/fixtures/persistence/v1/credentials.json`
- Modify: `src-tauri/src/settings.rs`
- Modify: `src-tauri/src/printers.rs`
- Modify: `src-tauri/src/connections/credentials.rs`

**Interfaces:**
- Consumes: `load_settings_from(&Path) -> Result<Settings, String>`,
  `load_printers_from(&Path) -> Result<PrintersFile, String>`, and
  `CredentialStore::file_backed(PathBuf)` plus `CredentialStore::get(&str)`.
- Produces: three immutable input fixtures under
  `src-tauri/tests/fixtures/persistence/v1/` and regression tests that future F1
  migration work must keep passing or deliberately supersede.

- [ ] **Step 1: Add failing compatibility tests that reference missing fixtures**

Add this test and helper inside `settings.rs`'s existing `tests` module:

```rust
fn persistence_fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/persistence/v1")
        .join(name)
}

#[test]
fn baseline_v1_settings_fixture_loads_through_the_storage_seam() {
    let dir = temp_dir();
    fs::create_dir_all(&dir).unwrap();
    fs::copy(persistence_fixture("settings.json"), settings_file_path(&dir)).unwrap();

    let loaded = load_settings_from(&dir).unwrap();

    assert_eq!(loaded.theme_mode, "farm3d-dark");
    fs::remove_dir_all(&dir).ok();
}
```

Add this test and helper inside `printers.rs`'s existing `tests` module:

```rust
fn persistence_fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/persistence/v1")
        .join(name)
}

#[test]
fn baseline_v1_printers_fixture_loads_through_the_storage_seam() {
    let dir = temp_dir();
    fs::create_dir_all(&dir).unwrap();
    let fixture = persistence_fixture("printers.json");
    fs::copy(&fixture, printers_file_path(&dir)).unwrap();

    let loaded = load_printers_from(&dir).unwrap();

    assert_eq!(loaded.schema_version, 1);
    assert_eq!(loaded.printers.len(), 2);
    assert_eq!(loaded.printers[0].id, "prn-f0-profile-only");
    assert_eq!(loaded.printers[0].connection, None);
    assert_eq!(loaded.printers[0].overrides.printable_height_mm, Some(245.0));
    assert_eq!(
        loaded.printers[0].overrides.extra.get("futureBaselineField"),
        Some(&serde_json::json!({ "preserved": true }))
    );
    let connected = &loaded.printers[1];
    assert_eq!(connected.id, "prn-f0-connected");
    assert_eq!(connected.connection.as_ref().unwrap().host, "moonraker.invalid");
    assert_eq!(
        connected.connection.as_ref().unwrap().credential_ref.as_deref(),
        Some("farm3d/printer/prn-f0-connected/apikey")
    );
    let raw = fs::read_to_string(fixture).unwrap();
    assert!(!raw.contains("F0_FIXTURE_SENTINEL"));
    fs::remove_dir_all(&dir).ok();
}
```

Add this test and helper inside `connections/credentials.rs`'s existing `tests`
module:

```rust
fn persistence_fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/persistence/v1")
        .join(name)
}

#[test]
fn baseline_file_backed_credentials_fixture_loads_through_the_storage_seam() {
    let dir = temp_dir();
    fs::create_dir_all(&dir).unwrap();
    fs::copy(
        persistence_fixture("credentials.json"),
        credentials_file_path(&dir),
    )
    .unwrap();
    let store = file_store(&dir);

    assert_eq!(
        store.get("farm3d/printer/prn-f0-connected/apikey").unwrap().as_deref(),
        Some("F0_FIXTURE_SENTINEL")
    );
    fs::remove_dir_all(&dir).ok();
}
```

- [ ] **Step 2: Run the focused tests and confirm the red state**

Run:

```sh
source "$HOME/.cargo/env" && cargo test --manifest-path src-tauri/Cargo.toml baseline_
```

Expected: all three new tests fail while copying their missing fixture files.

- [ ] **Step 3: Add the exact baseline fixtures**

Create `settings.json`:

```json
{
  "themeMode": "farm3d-dark"
}
```

Create `credentials.json`:

```json
{
  "farm3d/printer/prn-f0-connected/apikey": "F0_FIXTURE_SENTINEL"
}
```

Create `printers.json`:

```json
{
  "schemaVersion": 1,
  "printers": [
    {
      "id": "prn-f0-profile-only",
      "name": "F0 Profile-only Printer",
      "catalogRef": {
        "vendor": "TestVendor",
        "model": "Test Printer",
        "variant": "Test Printer 0.4 nozzle",
        "modelId": "TestVendor-TP",
        "printerVariant": "0.4"
      },
      "group": "Baseline Bay",
      "notes": "Profile-only migration fixture",
      "overrides": {
        "printableHeightMm": 245.0,
        "futureBaselineField": { "preserved": true }
      },
      "lastKnownGood": {
        "profile": {
          "bedShape": {
            "kind": "rectangular",
            "widthMm": 256.0,
            "depthMm": 256.0,
            "originXMm": 0.0,
            "originYMm": 0.0
          },
          "printableHeightMm": 256.0,
          "bedExcludeAreas": [],
          "defaultBedType": "4",
          "nozzleDiameterMm": [0.4],
          "nozzleType": "hardened_steel",
          "gcodeFlavor": "klipper",
          "hasAuxiliaryFan": true,
          "supportsAirFiltration": true,
          "supportsMultiFilament": false,
          "suggestedHostType": null
        },
        "catalogVersion": "v2.4.2",
        "resolvedAt": "2026-09-16T00:00:00Z"
      }
    },
    {
      "id": "prn-f0-connected",
      "name": "F0 Connected Printer",
      "catalogRef": {
        "vendor": "TestVendor",
        "model": "Test Printer",
        "variant": "Test Printer 0.4 nozzle",
        "modelId": "TestVendor-TP",
        "printerVariant": "0.4"
      },
      "group": "Baseline Bay",
      "notes": "Connection and credential-reference migration fixture",
      "connection": {
        "kind": "moonraker",
        "host": "moonraker.invalid",
        "port": 7125,
        "useTls": false,
        "credentialRef": "farm3d/printer/prn-f0-connected/apikey"
      }
    }
  ]
}
```

- [ ] **Step 4: Run the focused tests and confirm the green state**

Run:

```sh
source "$HOME/.cargo/env" && cargo test --manifest-path src-tauri/Cargo.toml baseline_
```

Expected: the three baseline fixture tests pass.

- [ ] **Step 5: Run the complete Rust suite**

Run:

```sh
source "$HOME/.cargo/env" && just test-rust
```

Expected: all Rust tests pass, including quarantine, Profile drift, variant
rebind, reconnect/backoff, status map, credential permissions, credential
cleanup, and fixture compatibility coverage.

- [ ] **Step 6: Commit the persistence baseline**

```sh
git add src-tauri/tests/fixtures/persistence/v1/settings.json \
  src-tauri/tests/fixtures/persistence/v1/printers.json \
  src-tauri/tests/fixtures/persistence/v1/credentials.json \
  src-tauri/src/settings.rs src-tauri/src/printers.rs \
  src-tauri/src/connections/credentials.rs
git commit -m "test: freeze F0 persistence fixtures"
```

### Task 2: Exercise the Existing Tauri Path

**Files:**
- Modify: `src-tauri/Cargo.toml`
- Modify: `src-tauri/src/lib.rs`
- Modify: `src-tauri/src/printers.rs`
- Modify: `src-tauri/src/connections/commands.rs`
- Modify: `src-tauri/src/connections/supervisor.rs`
- Create: `src-tauri/tests/f0_tauri_path.rs`
- Modify: `src/printers/printer-store.test.ts`

**Interfaces:**
- Consumes: existing `create_printer`, `set_printer_connection`, startup
  restoration, `printer_statuses`, `printer-status`, and frontend listener/
  backfill behavior.
- Produces: `ConnectionManager<R: tauri::Runtime>`,
  `restore_stored_connections<R: tauri::Runtime>(app: &tauri::AppHandle<R>,
  manager: &Arc<ConnectionManager<R>>)`, unchanged Tauri command names/payloads,
  and one mock-runtime IPC tracer covering create through restart/backfill.

- [ ] **Step 1: Enable Tauri's test runtime and write the failing IPC tracer**

Add to `src-tauri/Cargo.toml`:

```toml
[dev-dependencies]
tauri = { version = "2", features = ["test"] }
```

Create `src-tauri/tests/f0_tauri_path.rs` beginning with
`#![cfg(target_os = "linux")]`. F0 can safely isolate the existing
path-derived config seam with `XDG_CONFIG_HOME` only on Linux; candidate macOS
and Windows platforms must add native config-root isolation before enabling this
test there. The test must:

1. Serialize all mutations behind one test and point `XDG_CONFIG_HOME` at a
   unique temporary directory before creating either mock app.
2. Load `resources/printer-catalog.json` into `Arc<Catalog>`.
3. Build a Tauri mock app with `mock_builder()`, managed Catalog and
   `Arc<ConnectionManager<MockRuntime>>`, and handlers for `create_printer`,
   `list_printers`, `set_printer_connection`, and `printer_statuses`.
4. Create a mock webview and send a real `InvokeRequest` for `create_printer`
   with the first actual catalog variant's complete `CatalogRef`.
5. Send a real `InvokeRequest` for `set_printer_connection` using
   `kind: "moonraker"`, `host: "moonraker.invalid"`, port `7125`, TLS false,
   and an explicitly empty API key so no credential store or real secret is
   written.
6. Assert the IPC responses contain the created Printer and Connection but no
   `apiKey`, `secret`, or `F0_FIXTURE_SENTINEL` field/value.
7. Stop the first manager, drop the webview/app, create a second mock app on the
   same config root, and call the same `restore_stored_connections` helper used
   by production `run()`.
8. Wait for at most two seconds until the restarted manager's status map
   contains the Printer, then invoke `printer_statuses` through IPC and assert
   the Printer is present with `connecting` or a later error/offline state.
9. Stop the second manager's Printer task, drop its webview/app, remove the
   temporary directory, and restore the original `XDG_CONFIG_HOME`.

Use a local request helper with this exact signature so every operation passes
through Tauri IPC rather than calling command functions directly:

```rust
fn invoke(
    webview: &tauri::WebviewWindow<tauri::test::MockRuntime>,
    command: &str,
    body: serde_json::Value,
) -> Result<serde_json::Value, serde_json::Value>
```

Its `InvokeRequest` uses `InvokeBody::Json(body)`, callbacks `0` and `1`, URL
`tauri://localhost`, and `tauri::test::INVOKE_KEY`, then deserializes
`tauri::test::get_ipc_response` into `serde_json::Value`.

- [ ] **Step 2: Run the IPC tracer and confirm the red state**

Run:

```sh
source "$HOME/.cargo/env" && cargo test --manifest-path src-tauri/Cargo.toml --test f0_tauri_path
```

Expected: compilation fails because `ConnectionManager` and tracer-facing
commands are not generic and `restore_stored_connections` does not yet exist.

- [ ] **Step 3: Make the manager and tracer-facing commands runtime-generic**

In `connections/supervisor.rs`, change the manager to:

```rust
pub struct ConnectionManager<R: tauri::Runtime> {
    app: tauri::AppHandle<R>,
    tasks: Mutex<HashMap<String, JoinHandle<()>>>,
    statuses: Arc<StatusMap>,
}
```

Change the existing impl header to
`impl<R: tauri::Runtime> ConnectionManager<R>`, change `new` to accept
`tauri::AppHandle<R>`, and change `publish` to accept
`&tauri::AppHandle<R>`. Copy the existing method and function bodies byte for
byte; only type parameters change.

Add `<R: tauri::Runtime>` to every command that receives the now-generic
manager, plus the two tracer-facing Printer commands, preserving every command
name and payload. The complete signatures are:

```rust
pub fn list_printers<R: tauri::Runtime>(
    app: AppHandle<R>,
    catalog: tauri::State<Arc<Catalog>>,
) -> Result<Vec<ResolvedPrinter>, String>

pub fn create_printer<R: tauri::Runtime>(
    app: AppHandle<R>,
    catalog: tauri::State<Arc<Catalog>>,
    draft: PrinterDraft,
) -> Result<ResolvedPrinter, String>

pub async fn delete_printer<R: tauri::Runtime>(
    app: AppHandle<R>,
    manager: tauri::State<'_, Arc<ConnectionManager<R>>,
    id: String,
) -> Result<(), String>

pub async fn set_printer_connection<R: tauri::Runtime>(
    app: AppHandle<R>,
    catalog: tauri::State<'_, Arc<Catalog>>,
    manager: tauri::State<'_, Arc<ConnectionManager<R>>,
    id: String,
    submission: ConnectionSubmission,
) -> Result<ResolvedPrinter, String>

pub async fn clear_printer_connection<R: tauri::Runtime>(
    app: AppHandle<R>,
    catalog: tauri::State<'_, Arc<Catalog>>,
    manager: tauri::State<'_, Arc<ConnectionManager<R>>,
    id: String,
) -> Result<ResolvedPrinter, String>

pub fn printer_statuses<R: tauri::Runtime>(
    manager: tauri::State<Arc<ConnectionManager<R>>>,
) -> HashMap<String, PrinterStatus>
```

Make both private config-directory helpers and the credential-store helper
generic over the same runtime:

```rust
fn app_config_dir<R: tauri::Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String>
fn config_dir<R: tauri::Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String>
fn store<R: tauri::Runtime>(app: &AppHandle<R>) -> Result<CredentialStore, String>
```

Search every `ConnectionManager` and `AppHandle` use after the conversion and
make any manager-bearing type explicit about `R`. Do not alter persistence,
credential, adapter, or status semantics.

- [ ] **Step 4: Compile-check the complete runtime conversion**

Run:

```sh
source "$HOME/.cargo/env" && cargo check --manifest-path src-tauri/Cargo.toml --lib --bins
```

Expected: the production library and binaries compile with no missing runtime
parameter or concrete `AppHandle` mismatch. The still-red IPC test is compiled
after the restoration helper lands.

- [ ] **Step 5: Extract and reuse startup restoration**

Move the existing Printer/credential/supervisor startup loop from
`src-tauri/src/lib.rs` into this public generic helper in `lib.rs`:

```rust
pub fn restore_stored_connections<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    manager: &Arc<ConnectionManager<R>>,
) {
    if let Ok(dir) = app.path().app_config_dir() {
        if let Ok(file) = printers::load_printers_from(&dir) {
            let store = connections::credentials::CredentialStore::detect(dir);
            for stored in &file.printers {
                let Some(config) = stored.connection.clone() else { continue };
                let api_key = match config.credential_ref.as_deref() {
                    None => None,
                    Some(key) => match store.get(key) {
                        Ok(found) => found,
                        Err(e) => {
                            eprintln!(
                                "farm3d: cannot read the stored credential for {}: {e}",
                                stored.id
                            );
                            manager.report_error(
                                &stored.id,
                                format!("Could not read this printer's stored credential: {e}"),
                            );
                            continue;
                        }
                    },
                };
                manager.start(stored.id.clone(), config, api_key);
            }
        }
    }
}
```

Production `run()` must call this helper before `app.manage(manager)`. The IPC
tracer calls the same helper after constructing its restarted manager. Preserve
the existing behavior that unreadable credentials create a terminal status
error rather than an unauthenticated reconnect loop.

- [ ] **Step 6: Pin frontend listener-before-backfill ordering**

Extend the existing `applies a printer-status event straight off the Rust event
channel` test in `src/printers/printer-store.test.ts` with a `calls: string[]`.
Push `"listen:printer-status"` in the event mock and `"invoke:<command>"` in the
invoke mock, then assert after `startStatusListener()`:

```ts
expect(calls.slice(-2)).toEqual([
  "listen:printer-status",
  "invoke:printer_statuses",
]);
```

- [ ] **Step 7: Run focused and complete tests**

Run:

```sh
source "$HOME/.cargo/env" && cargo test --manifest-path src-tauri/Cargo.toml --test f0_tauri_path
npm test -- src/printers/printer-store.test.ts
source "$HOME/.cargo/env" && just test-rust
just test
```

Expected: the IPC tracer and listener-order regression test pass, followed by
the complete Rust and frontend suites.

- [ ] **Step 8: Commit the Tauri tracer**

```sh
git add src-tauri/Cargo.toml src-tauri/Cargo.lock src-tauri/src/lib.rs \
  src-tauri/src/printers.rs src-tauri/src/connections/commands.rs \
  src-tauri/src/connections/supervisor.rs src-tauri/tests/f0_tauri_path.rs \
  src/printers/printer-store.test.ts
git commit -m "test: trace the F0 Tauri restart path"
```

### Task 3: Record Baseline and Platform Evidence

**Files:**
- Create: `docs/superpowers/baselines/2026-09-16-f0-baseline.md`
- Create: `docs/superpowers/baselines/2026-09-16-supported-platforms.md`

**Interfaces:**
- Consumes: Task 1's fixtures/tests; command registration in
  `src-tauri/src/lib.rs`; listener/backfill ordering in `src/App.tsx` and
  `src/printers/printer-store.ts`; startup supervision in `src-tauri/src/lib.rs`;
  current test names and current host/tool/package output.
- Produces: the evidence records referenced by F1 and every later phase that
  makes platform-specific claims.

- [ ] **Step 1: Capture reproducible environment and repository facts**

Run each command from the repository root and retain its exact output for the
baseline report:

```sh
git rev-parse HEAD
git status --short
uname -a
node --version
npm --version
just --version
source "$HOME/.cargo/env" && rustc --version
source "$HOME/.cargo/env" && cargo --version
npm run tauri -- --version
```

Expected: every version command succeeds. The status output may include the
known unrelated `.devcontainer/`; the report identifies it as excluded rather
than treating it as F0 content.

- [ ] **Step 2: Run all automated gates**

Run:

```sh
just build
just test
source "$HOME/.cargo/env" && just test-rust
```

Expected: all commands pass. Record the command, UTC execution timestamp, exit
status, and test counts reported by Vitest/Cargo. If a gate exposes a product
defect, add a focused failing regression test, implement the minimum fix, rerun
all three gates, and list the fix under `Defects discovered during F0`. Commit a
reproduced defect separately before continuing: first record every exact
regression-test and production path under `Defects Discovered During F0`, then
run one `git add -- path` invocation per recorded path. Do not derive this list
from all modified files, and do not stage any path that was already modified
before the defect work. Inspect `git diff --cached`, confirm it contains only
the recorded defect paths, and run
`git commit -m "fix: address defect found during F0 verification"`. If the fix
necessarily changes another production path, name it in the report before
staging it; never use `git add .`, `git add -A`, or `git add -u`.

- [ ] **Step 3: Exercise and inventory the frontend baseline**

Start `just web` on `127.0.0.1:1420`, wait up to 30 seconds for
`http://127.0.0.1:1420/`, request `/` and the catalog resource, require HTTP 200
from both, then terminate the Vite process. The exact URLs are
`http://127.0.0.1:1420/` and
`http://127.0.0.1:1420/src-tauri/resources/printer-catalog.json`. Record the
startup/request result without calling it Tauri evidence.

In the report, inventory each required surface with its source and existing
test evidence: `AppShell`/`ActivityBar`, `PrinterDashboard`, Add dialog, Status/
Profile/Connection detail panels, `SettingsMenu`/`ThemePopover`, `ModelLibrary`
scaffold, `printer-store` web fallback, root error banner, Connection inline
probe errors, and every current `*.test.ts`/`*.test.tsx` file. Record loading,
empty, error, and web-fallback behavior where each exists; record absent states
as gaps rather than inferring them.

- [ ] **Step 4: Build the Linux package and identify its artifacts**

Run:

```sh
source "$HOME/.cargo/env" && just package
```

Then list the regular files beneath `src-tauri/target/release/bundle/` and record
their names and sizes in the baseline report. Expected: packaging succeeds on
the Linux x86_64 development host and produces at least one Linux bundle plus
`src-tauri/target/release/farm3d`.

- [ ] **Step 5: Exercise the packaged executable without overclaiming UI proof**

If `$DISPLAY` or `$WAYLAND_DISPLAY` is set, launch
`src-tauri/target/release/farm3d`, confirm the process remains alive for ten
seconds, terminate it normally, and record this as a packaged-executable smoke
launch. If neither display variable is set, invoke the executable once, capture
the display-initialization failure, and record `Package built; GUI launch not
verified because no display server was available`. That outcome leaves Linux
`Candidate, unverified` and F0 open; resolve the local display/tooling blocker
and rerun before opening the PR.

This is not an installed-bundle claim and not visual acceptance. Do not install
system packages or request elevated privileges solely to strengthen F0 evidence.

- [ ] **Step 6: Record live Moonraker absence**

Confirm `FARM3D_F0_MOONRAKER_URL` remains absent. The planning environment had no
authorized live endpoint, so F0 performs no network probing and records exactly:

```text
Not run: no reachable representative Moonraker instance was available to F0.
Protocol fixtures and unit tests passed but are not live-hardware evidence.
```

If the variable unexpectedly exists at execution time, do not invent an ad hoc
probe or expose it in logs. Record that an endpoint was present but live exercise
was deferred to A0.1 because F0 has no approved bounded disconnect/reconnect
harness; this still satisfies F0's explicit evidence-or-absence gate without
claiming live validation.

- [ ] **Step 7: Capture outputs and run the preliminary sentinel scan**

Capture build/test/package and packaged-executable stdout/stderr under
`/tmp/farm3d-f0/`. Search `src/`, `src-tauri/`, and those captured logs for
`F0_FIXTURE_SENTINEL`. The only permitted repository matches are:

```text
src-tauri/tests/fixtures/persistence/v1/credentials.json
src-tauri/src/connections/credentials.rs
src-tauri/src/printers.rs
src-tauri/tests/f0_tauri_path.rs
```

The credentials test intentionally reads the sentinel; the Printers and IPC
tests assert it is absent. No production source, frontend state, serialized IPC
response, event/error assertion, or captured output may contain the sentinel.
Diagnostics do not exist in F0; record that fact rather than claiming a
diagnostics scan.

- [ ] **Step 8: Write the baseline report with exact evidence**

Create `docs/superpowers/baselines/2026-09-16-f0-baseline.md` with these
completed sections and no blank evidence cells:

```markdown
# F0 Baseline

## Scope and Provenance
## Environment
## Frontend Inventory
## Backend Inventory
## Command and Event Inventory
## Printer Tracer
## Persisted Files
## Automated Verification
## Package and Launch Evidence
## Live Moonraker Evidence
## Known Gaps
## Defects Discovered During F0
## Acceptance Checklist
```

The report must state these source-verified facts:

- `App` loads Printers before registering the status listener because unknown-id
  status is dropped.
- `startStatusListener` registers `printer-status` before invoking
  `printer_statuses`, closing the listener/backfill race.
- `printer-status` is the only frontend event; enumerate all 21 registered Tauri
  commands from `src-tauri/src/lib.rs`.
- Startup loads the catalog, creates `ConnectionManager`, loads
  `printers.json`, resolves credentials, and starts supervision for each stored
  Connection before managing the manager state.
- `settings.json` defaults on missing/corrupt input; `printers.json` creates an
  empty schema-v1 file when absent and quarantines corrupt input; credentials
  use keychain when available and an explicit owner-only file fallback.
- Connection save writes a new secret before persisting its reference, writes
  Printer state before starting supervision, and explicit clear/delete removes
  credentials according to the existing ordering documented in source.
- The Printer tracer separates source-traced behavior, Task 2's Tauri
  mock-runtime IPC evidence, pure/unit-test evidence, packaged-executable
  evidence, and live-hardware evidence. It identifies mock-runtime IPC as below
  real-WebView/installed-bundle evidence rather than calling it full desktop E2E.
- Profile drift, variant rebind, quarantine, supervisor restart/backoff, status
  backfill, credential cleanup, and missing/partial Moonraker fields each point
  to the exact existing test/module evidence.
- The three Task 1 fixtures are the canonical F0 migration inputs for F1.

- [ ] **Step 9: Write the supported-platform matrix**

Create `docs/superpowers/baselines/2026-09-16-supported-platforms.md` with:

```markdown
# Supported Platforms

## Decision
## Evidence Levels
## Matrix
## Promotion Requirements
## F0 Evidence
```

Use this matrix, replacing the Linux status/evidence cells with the exact result
from Steps 4-5. `Supported` is permitted only when both pass; otherwise use
`Candidate, unverified` and leave F0 open:

| Platform | v1 release status | F0 evidence | Required before platform claim |
|---|---|---|---|
| Linux x86_64 | Supported only after successful F0 package and launch; otherwise Candidate, unverified | Exact F0 result | Clean build/test/package; packaged or installed executable launch; later feature-specific installed-bundle checks |
| Windows x86_64 | Candidate, unverified | Not run in F0 | Native clean build/test/package; installed launch; credential-store and later permission/notification/process checks |
| Windows arm64 | Candidate, unverified | Not run in F0 | Native clean build/test/package; installed launch; credential-store and later permission/notification/process checks |
| macOS x86_64 | Candidate, unverified | Not run in F0 | Native clean build/test/package; signed/notarized installation decision; launch; Keychain and later permission/notification/process checks |
| macOS arm64 | Candidate, unverified | Not run in F0 | Native clean build/test/package; signed/notarized installation decision; launch; Keychain and later permission/notification/process checks |

Define evidence levels explicitly: source/configuration, automated tests, package
build, packaged/installed launch, live external-system exercise. State that
`"targets": "all"` means all bundle types available on the current build host,
not all operating systems.

- [ ] **Step 10: Cross-check the reports against source and acceptance criteria**

Confirm every command/event name matches `src-tauri/src/lib.rs`, every ordering
claim matches the cited function, every test claim names an existing test, every
fixture path exists, the platform matrix has no unsupported claim, and all five
F0 exit-gate items are checked in the baseline report. Then search both newly
created baseline documents and `/tmp/farm3d-f0/` for
`F0_FIXTURE_SENTINEL`; require zero matches before committing.

- [ ] **Step 11: Commit the evidence documents**

```sh
git add docs/superpowers/baselines/2026-09-16-f0-baseline.md \
  docs/superpowers/baselines/2026-09-16-supported-platforms.md
git commit -m "docs: record F0 baseline evidence"
```

### Task 4: Independently Verify and Close F0

**Files:**
- Modify if findings require correction:
  `docs/superpowers/baselines/2026-09-16-f0-baseline.md`
- Modify if findings require correction:
  `docs/superpowers/baselines/2026-09-16-supported-platforms.md`
- Modify only for a reproduced defect: files named by the failing regression
  test, with the fix recorded in the baseline report.

**Interfaces:**
- Consumes: Tasks 0-3 and the F0 exit gate in the umbrella implementation
  approach.
- Produces: independently reviewed F0 evidence, a clean final verification run,
  and a branch ready for pull-request review.

- [ ] **Step 1: Run independent standards and spec reviews**

Dispatch fresh reviewers that did not implement Tasks 1-2:

1. Standards review against `AGENTS.md`, repository conventions, fixture
   secrecy, and evidence accuracy.
2. Spec review against Phase F0 lines 261-315 and this plan's Global
   Constraints.

Expected: each reviewer returns either `APPROVED` or concrete findings with
file/line references. Resolve all high- and medium-severity findings; resolve or
explicitly document low-severity evidence limitations.

- [ ] **Step 2: Re-run the final gate from a clean command invocation**

Run:

```sh
just build
just test
source "$HOME/.cargo/env" && just test-rust
source "$HOME/.cargo/env" && just package
git diff --check
git diff --check origin/main...HEAD
```

Expected: every command exits zero. Update only the report's verification
timestamp/count/artifact facts if the final run differs. Before running the
commands, recreate `/tmp/farm3d-f0/final/`; capture each command's complete
stdout/stderr in its own file there using `tee` under `set -o pipefail`, so a
failed command remains a failed gate. Launch the exact
`src-tauri/target/release/farm3d` produced by this final `just package`, require
it to remain alive for ten seconds on the available display, terminate it
normally, capture its stdout/stderr in `/tmp/farm3d-f0/final/launch.log`, and
update the launch evidence with the final artifact timestamp.
Then repeat the sentinel scan across `src/`, `src-tauri/`, both baseline
documents, and the fresh `/tmp/farm3d-f0/final/` logs, enforcing Task 3 Step
7's exact allowlist, and rerun both diff-check commands.

- [ ] **Step 3: Verify the F0 acceptance checklist explicitly**

Mark F0 complete only when all are true:

- Baseline report exists and source claims are cited.
- Complete automated suite passes.
- Settings, Printer schema-v1, and file-backed credential fixtures load through
  the real storage seams.
- Supported-platform matrix names Linux x86_64 as the sole initial supported
  platform only with successful F0 package/launch evidence, and leaves all
  untested platforms unverified.
- Live Moonraker evidence includes versions/results, or the report contains the
  exact no-instance statement from Task 3 Step 6.
- No real credential or unrelated `.devcontainer/` content is included.

- [ ] **Step 4: Commit review corrections, if any**

Inspect `git status --short`, then stage each corrected path explicitly from this
closed allowlist: the two baseline documents, Task 1's three Rust modules and
three fixtures, Task 2's Rust/TypeScript modules and IPC test, and the four
approved Task 0 documents. A reproduced defect's exact production/regression
paths are permitted only when they are named under `Defects Discovered During
F0`; normally they were already committed in Task 3 Step 2. Run
`git commit -m "docs: address F0 verification review"`.

Skip this commit when reviewers found nothing requiring changes; do not create
an empty commit.

- [ ] **Step 5: Prepare the pull request evidence**

Inspect `git status`, `git diff`, `git log --oneline -10`, the upstream tracking
branch, and the full branch diff from `origin/main`. Confirm only approved v1
documents, F0 fixtures/tests, and F0 evidence are included. Push the branch and
open a PR whose body contains:

```markdown
## Summary
- freeze current settings, Printer, and file-backed credential formats as migration fixtures
- document the F0 command/event/restart and automated verification baseline
- declare and evidence the initial supported-platform matrix

## Verification
- `just build`
- `just test`
- `source "$HOME/.cargo/env" && just test-rust`
- `source "$HOME/.cargo/env" && just package`
- packaged executable launch result from the F0 report
- live Moonraker result or explicit no-instance outcome from the F0 report
```

Expected: the PR targets `main`, includes the baseline commit/reference and F0
acceptance status, and its URL is returned to the user.
