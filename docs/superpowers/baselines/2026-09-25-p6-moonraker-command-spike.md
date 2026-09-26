# P6 Moonraker Command Spike

**Status:** Approved by the controller (owner-delegated), 2026-09-25, after task review and one fix round.

Task 3 of `docs/superpowers/plans/2026-09-25-p6-connection-command-capabilities.md`.
It ran on 2026-09-25 on the Linux x86_64 development host. Every write
(upload, print control, restart, fault) went to the loopback simulators
(`sim/`). The owner's Snapmaker U1 was only queried, through HTTP GET and
read-only WebSocket JSON-RPC calls. The probes were throwaway Python scripts
outside the repository and are not committed.

Task 4 turns these findings into the P6 spec. The decisions it has to take
are listed under [Decisions for the spec](#decisions-for-the-spec).

## Results

"Real host" means the owner's Snapmaker U1. "Not run" means the gate needs
a write, and agents never write to a real printer.

| Gate | Simulator | Real host (read-only) |
|---|---|---|
| A. HTTP auth | PASS | PASS: no key needed for reads |
| B. Upload without start | PASS | Not run |
| C. Identity | PASS | PASS for field shapes. Download timing Not run. |
| D. Interrupted upload | PASS for both cases. The mid-body case used a scratch upstream toxic, not Task 12's helper. | Not run |
| E. Control | PASS. The matrix is recorded, with hazards, below. | Not run |
| F. Start evidence | PASS | PASS for shapes and clock offset |
| G. Host restart | PASS | Not run |
| H. Capability detection | PASS on `moonraker`, `moonraker-multi`, and `variant no-bed` | PASS |
| I. Clean start state | PASS | PASS: `complete` persisted for about 68 hours |

PASS means the gate's question was answered with evidence. It does not mean
Moonraker behaved as the plan assumed. Gates E, F and G found behavior that
changes D5 (see the decisions).

## Versions and run evidence

| Item | Simulator | Real host |
|---|---|---|
| Moonraker | `v0.11.0-1-g1cfb0c4-prind`, API `1.5.0` | `1.5.2` (Snapmaker's fork), API `1.4.0` |
| Klipper | `v0.13.0-770-gce7002bed-prind` | `software_version` `1.5.2.13_20260722102206` (Snapmaker firmware string) |
| Images | `docker.io/mkuf/moonraker:v0.11.0-1-g1cfb0c4@sha256:c398e6e95def8f4112128ddf51d6dab3a7766e6ea28691e7b42f3bb98622a26d`; `localhost/farm3d-simulavr:prind-80600f7-klipper-ce7002b` (image id `7c3347af30ce…`, prind `80600f7d6f0d`, Klipper `ce7002bedf37`); `ghcr.io/shopify/toxiproxy:2.12.0@sha256:9378ed52a28bc50edc1350f936f518f31fa95f0d15917d6eb40b8e376d1a214e` | — |
| Engine | rootless podman, host networking | — |

The probes ran against the simulators started by `just sim-up` at repo
commit `f6535b2`. `sim/simctl status` reported every simulator ready
(`moonraker` and `moonraker-multi` `klippy_state=ready`, toxiproxy
`2.12.0`). After the probes, `sim/simctl reset && just test-sim` recorded
`src-tauri/target/sim-runs/20260925T204602Z/manifest.json`. It shows the
same images, pins and reported versions, and all 25 simulator tests passed.
The probes are scratch scripts, so that manifest records the environment,
not the probes themselves.

## Real-host calls made

This is every call made to the owner's Snapmaker U1. All were reads, with no
key and no request body. No file was downloaded, and nothing was posted.

| Transport | Call | Count | On the allowed list? |
|---|---|---|---|
| HTTP GET | `/server/info` | 6 (1 capture, 5 `Date`-header clock samples) | yes (`server.info`) |
| HTTP GET | `/printer/objects/list` | 1 | yes |
| HTTP GET | `/printer/objects/query?print_stats&virtual_sdcard&webhooks&pause_resume&idle_timeout&extruder&extruder1&extruder2&extruder3&heater_bed&toolhead` | 1 | yes |
| HTTP GET | `/server/files/list?root=gcodes` | 1 | yes |
| HTTP GET | `/server/files/metadata?filename=<existing file>` | 1 | yes |
| HTTP GET | `/server/history/list?limit=5` | 1 | yes |
| HTTP GET | `/server/webcams/list` | 1 | yes |
| HTTP GET | `/printer/info` | 1 | **no**, read-only |
| HTTP GET | `/access/info` | 1 | **no**, read-only |
| HTTP GET | `/server/files/roots` | 1 | **no**, read-only |
| HTTP GET | `/server/files/directory?path=gcodes&extended=true` | 1 | **no**, read-only (returns the same metadata as `server.files.metadata`, for every file) |
| WebSocket | `server.info` | 2 | yes |
| WebSocket | `printer.objects.query` (`print_stats` state and filename; `toolhead.extruder` and `configfile.settings`) | 2 | yes |
| WebSocket | `printer.objects.list` | 1 | yes |
| WebSocket | `server.webcams.list` | 1 | yes |
| WebSocket | `server.history.list` (`limit: 1`) | 1 | yes |

Fix round 1 made no real-host calls.

## Gate A: HTTP auth

**Simulator (`sim/simctl variant moonraker apikey`):**

- With no key, or a wrong key, every HTTP endpoint tried answered **401**.
  That was `/server/info`, `/server/files/list`, `/server/files/metadata`,
  `/server/history/list`, `/server/webcams/list`, `/printer/objects/list`,
  the upload, the file download (`GET /server/files/gcodes/...`), and
  `POST /printer/print/start`. With the right `X-Api-Key` they answered 200.
  Only `/access/info` answered without a key (200, `"trusted": false`).
- The 401 body is
  `{"error":{"code":401,"message":"Unauthorized","traceback":"..."}}`. The
  traceback names Moonraker's internal file paths and ends in
  `HTTP 401: Unauthorized (Invalid API Key)` for a wrong key. A seeded key
  (`seeded-secret-XYZ123`) did **not** appear in the 401 body or in the
  WebSocket error.
- **The WebSocket upgrade succeeds without a key** (`101`). Every method
  then fails with the JSON-RPC error `{"code": -32602, "message":
  "Unauthorized"}`. With `X-Api-Key` on the upgrade request, the methods
  succeed. Without it, `server.connection.identify` with an `api_key`
  param also authorizes the connection.

**Real host:** `/access/info` reports `login_required: false` and
`trusted: true`. Every read (HTTP and WebSocket) worked with no key.

## Gate B: Upload without start

`POST /server/files/upload` (multipart: `root=gcodes`, `path=farm3d`,
`file` named `<uuid>.gcode`, no `print` field):

- **201** with
  `{"action":"create_file","item":{"modified":<epoch float>,"size":651,"permissions":"rw","path":"farm3d/<uuid>.gcode","root":"gcodes"},"print_started":false,"print_queued":false}`.
  It also sends a `Location` header. The `farm3d/` directory was created
  on demand.
- `print_stats` and `virtual_sdcard` were identical before and 1.5 s after
  the upload (`standby`, empty filename).
- **`checksum` is accepted.** A correct lower-case SHA-256 gave 201. An
  upper-case one also gave 201, because Moonraker lower-cases both sides.
- **A wrong checksum is rejected with 422** (`"message": "Unprocessable
  Entity"`). The traceback carries `File checksum mismatch: expected …,
  calculated …`. Nothing is left at the destination (metadata 404, not in
  the list). A malformed checksum (`abc`) is also 422.
- Re-uploading to the same path replaces the file with 201, and `action` is
  still `create_file`.
- An upload over the file that is printing or paused is refused with
  **403** (`File is loaded, upload not permitted`).
- Source reading (Moonraker's `FileUploadHandler.post` and
  `finalize_upload`): the request streams into a temporary file and the
  SHA-256 is computed while streaming. On a checksum mismatch or a 403 the
  temporary file is removed and the destination is never touched. Only
  after that is the file moved into place. Form fields Moonraker does not
  register are ignored, so a Moonraker without checksum support would
  presumably accept the upload and silently skip the check. That is
  inferred from the parser, not tested.

## Gate C: Identity

**Simulator.** `server.files.list` gives `path`, `modified`, `size`, and
`permissions`. `server.files.metadata` adds `uuid`, `slicer`,
`slicer_version`, `gcode_start_byte`, `gcode_end_byte`, `file_processors`,
`print_start_time`, `job_id`, and `filename`. **No field is a content
hash.** The metadata `uuid` is assigned when Moonraker parses the file, not
derived from its bytes. It changed when the file was overwritten. It stayed
the same across a Klipper restart and a Moonraker restart.

**Download-and-hash, 20 MB (20,971,536 bytes).** The file was uploaded with
its checksum (201 in 0.29 s), then streamed from
`GET /server/files/gcodes/<path>` and hashed in 1 MiB chunks. The digest
matched every time:

| Path | Runs (s) |
|---|---|
| Through Toxiproxy (27125) | 0.029, 0.026, 0.027 |
| Direct (27126) | 0.020, 0.020, 0.021 |

This is loopback, so it only proves the method works. It says nothing about
LAN throughput. The download response has `Content-Length`,
`Last-Modified`, and `Accept-Ranges: bytes`, but no `ETag` or digest header.

**Real host.** The field shapes are the same, and there is still no hash.
`server.files.list` gives `path`, `modified`, `size`, `permissions`.
`server.files.metadata` has the same keys plus Snapmaker/Orca extras
(per-tool arrays such as `filament_used_mm`, `nozzle_temp`, and
`nozzle_diameter_list`, plus thumbnails). The largest existing file is about
21 MB. **Download timing was not run.** A file download is not in the
allowed read-only call list, so there is no LAN measurement.

## Gate D: Interrupted upload

The response-lost case used the same toxic as the harness's
`Toxiproxy::cut_after(Moonraker, 0)`: `limit_data`, downstream, 0 bytes.
The mid-body case used a scratch `limit_data` toxic on the **upstream**
stream at half the body size, added through the Toxiproxy HTTP API. Each
size ran once. The toxic was removed as soon as the client saw the error,
and `host_path` was checked **once, 1.0 s later**. That single check does
not show whether a file could still appear later. The client-timeout runs
below test that.

| Case | Body | Client saw | At `host_path` afterwards |
|---|---|---|---|
| Response lost | 4 KB, 2 MB, 20 MB | `RemoteDisconnected` after 0.20–0.30 s | **Complete file**, size equal to the upload, in all 3 |
| Mid-body cut, with checksum | 4 KB | `RemoteDisconnected` | Nothing (metadata 404) |
| Mid-body cut, with checksum | 2 MB, 20 MB | `ConnectionResetError` | Nothing |
| Mid-body cut, no checksum | 4 KB, 2 MB, 20 MB | as above | Nothing |

- **The destination never holds a partial file** after an interrupted body.
  Moonraker writes to the destination only after the whole body arrived.
- **Partial temp files are orphaned.** Every mid-body cut left a
  `moonraker.upload-<n>.mru` in Moonraker's temp directory, at 0 bytes,
  about 1.05 MB, and about 10.5 MB. They survive a Moonraker container
  restart (Gate G). The API cannot see or delete them. Moonraker has no
  connection-close cleanup for them.
- **The client cannot tell the two cases apart.** A 4 KB upload whose
  response was lost and a 4 KB upload cut mid-body both raised
  `RemoteDisconnected`. The first was applied and the second was not.

**Uploads the client abandons on a timeout** (fix round 1). The client
used a 3 s timeout and then closed its socket. `host_path` was then polled
through the direct port every 0.25 s for 45 s, and again after the toxic
was removed. Each case ran twice with the same result:

| Toxic | Body | Client | `host_path` over time (from dispatch) |
|---|---|---|---|
| Downstream latency 10 s (the response is delayed) | 20 MB | `TimeoutError` at 3.06 s | Already present at 3.06 s, full size, until 48.5 s |
| Upstream bandwidth 1 MB/s (the body is still in flight) | 20 MB | `TimeoutError` at 3.01 s | Absent from 3.01 s to 48.5 s. A 5.5 MB orphan `.mru` was left each time. |
| Upstream latency 10 s (the proxy holds the whole body) | 2 MB | `TimeoutError` at 3.00 s | **Absent at 3.01 s, then present at 10.29 s**, full size, until 48.5 s |

The last row is the important one. **The file appeared 7.3 s after the
client had given up**, while it was absent at the client's first check.
The proxy had buffered the whole body and delivered it after the client
closed. A real network path with buffering (a proxy, or kernel socket
buffers on a slow link) can do the same. So "absent" right after a timeout
does not prove the upload was not applied.

## Gate E: Control

The probe used HTTP `POST /printer/print/{start,pause,resume,cancel}` and
WebSocket `printer.print.*`, both through the proxy. Files held only
comments and `M117`. Every heater target was checked as 0 before each
start. `LONG` is a 300 MB comments-and-`M117` file that takes about 30 s to
run. `SMALL` is two lines. Each row below was seen in two full runs of
the matrix, with Klipper restarted before each. The "paused, start" row
with the *same* file comes from an earlier partial run.

| From | Verb | Response (HTTP / WS) | Effect |
|---|---|---|---|
| standby, complete, cancelled | cancel | 200 `ok` / `"ok"` | none |
| standby, complete, cancelled | resume | 200 `ok` / `"ok"` | none |
| standby, complete, cancelled | pause | 200 `ok` / `"ok"` | **sets `pause_resume.is_paused = true`**, and `print_stats` stays the same. A following cancel or resume clears it. |
| standby | start, missing file | 400 `Unable to open file` (both) | none |
| standby | start | 200 `ok` | `printing` within 0.05 s |
| complete | start | 200 `ok` | `printing`, new history job |
| cancelled | start | 200 `ok` | runs, new history job |
| printing | start, same or another file | 400 `SD busy` (both) | none |
| printing | resume | 200 `ok` | none |
| printing | pause | 200 `ok` | `paused` |
| printing | cancel | 200 `ok` | `cancelled`, `file_position` 0 |
| paused | pause | 200 `ok` | none |
| paused | resume | 200 `ok` | `printing` |
| paused | cancel | 200 `ok` | `cancelled` |
| paused | **start** | **200 `ok`** | **Starts the named file from byte 0** (same file or another). `is_paused` stays `true`, so a later pause is a no-op (`Print already paused`) and a later resume answers `SD busy`. |
| Klippy `shutdown` | any | HTTP 400 `"message": "Unknown"`. WS 400 with the full text (`…Printer is shutdown`). | none |
| Klippy `startup` | any | HTTP 400 `"Unknown"`. WS 400 `Printer is not ready…`. | none |
| Klipper stopped (`disconnected`) | any | 503 `Klippy Host not connected` (both) | none |

**Start that times out on the client.** A `G4 P6000` dwell was sent from
the console first. It is not motion or heating, and not part of any
started file. Then `start LONG` was sent with a 2 s client timeout. The
client timed out. Klipper ran `SDCARD_PRINT_FILE` **36.5 s after dispatch**
(the `server.gcode_store` response timestamp), once the dwell released the
G-code queue. simulavr runs slower than real time, which stretched the
dwell. A cancel sent 30 s after dispatch was queued behind the start, and
the start ran just before it. Because the print ran for less than
Moonraker's status batch, **no history job was recorded and `print_stats`
went from `cancelled` to `cancelled`**. In a second run with no client
timeout, the same queued start answered `ok` after 15.8 s. While it waited,
`idle_timeout.state` was `Printing` and `print_stats` stayed `cancelled`.

The same run shows that **pause, resume and cancel are queued the same
way**. The cancel sent 30 s after dispatch waited behind the queued start,
and only took effect after it.

**Queued start, then a Klipper restart** (fix round 1). This ran twice.
Each time a `G4 P20000` dwell was sent, then `start LONG` 0.3 s later with
a 120 s client timeout. The Klipper container was restarted 3.76 s after
dispatch, while `idle_timeout.state` was `Printing` and `print_stats` was
`standby`. Both waiting requests (the dwell and the start) answered **503
`Klippy Disconnected`** at 3.81–3.82 s. For 60 s after the restart,
`print_stats` stayed `standby` with an empty filename, and no history job
appeared. The `server.gcode_store` shows the `SDCARD_PRINT_FILE` command
with no response after it. So in this test **the queued start was dropped,
not run late**. This is one Klipper and Moonraker version, and it does not
show that a start which already ran briefly before the restart leaves any
trace.

The classification is in the decisions below.

## Gate F: Start evidence

This ran 6 times: HTTP 3 times and WebSocket 3 times, each with a
`limit_data` downstream-0 cut. The WebSocket cut was added after the
connection opened.

- The client saw `RemoteDisconnected` (HTTP) or a closed socket
  (WebSocket) within 0.001–0.003 s.
- **The start was applied every time.** `print_stats` went from
  `standby` or `cancelled` straight to `printing` with the filename equal
  to `host_path`. It was first seen 0.048–0.049 s after dispatch.
- Every time there was a **new history job** for `host_path`, with status
  `in_progress`. Its `start_time − dispatch time` was **0.048–0.049 s**.

**Clock skew.** The simulator containers share the development host's
kernel clock, so their skew is 0. The difference measured above is
Moonraker's detection delay. Moonraker's `job_state` infers the start from
batched `print_stats` updates and stamps the job with its own wall clock.
On the real host, five read-only GETs compared the HTTP `Date` header
(1 s resolution, truncated) with the local clock at the midpoint of the
request. Together they bound the offset to **between −0.08 s and +0.01 s**
at that time. Nothing shows how the host's clock drifts over time.

**Real-host shapes.** `server.history.list` jobs have `job_id` (six hex
digits), `user`, `filename`, `status`, `start_time`, `end_time`,
`print_duration`, `total_duration`, `filament_used`, `metadata`,
`auxiliary_data`, and `exists`. That is the same key set as the simulator.
`print_stats` has `filename`, `state`, `message`, `print_duration`,
`total_duration`, `filament_used`, and `info.{current_layer,total_layer}`,
plus a Snapmaker-only `exception` object. `virtual_sdcard` adds a
Snapmaker-only `pl_env_valid`.

**History attribution is unreliable in three simulator cases.** Moonraker's
`job_state` source explains all three:

1. **Start from `complete` of the same short file:** no history job, and no
   `print_stats` change. `SMALL` finished within one status batch, so
   Moonraker saw `complete` → `complete`. This was seen 2 out of 2 times.
2. **Start from `paused` with a different file that finished quickly:** no
   new job. The *old* job (`LONG`) was closed as `completed`, although
   `SMALL` ran. This was seen 2 out of 2 times.
3. **Moonraker restarted mid-print** (Gate G): the job was closed as
   `server_exit` while Klipper kept printing, and no job ever recorded the
   rest of that print.

Job ids come from an SQLite `INTEGER PRIMARY KEY` (without `AUTOINCREMENT`).
They increase with each job, but deleting the newest jobs lets their ids be
reused.

## Gate G: Host restart

| Restart | During | Result |
|---|---|---|
| Moonraker | 20 MB upload at 2 MB/s (bandwidth toxic), 3 s in | Client got `ConnectionResetError`. **Nothing at `host_path`.** A 6.4 MB orphan `.mru` stayed in the temp directory after the restart. |
| Klipper | the same upload | **Upload unaffected:** 201, full size |
| Klipper | print | `print_stats` → `standby`, filename `""`. The history job was closed as `klippy_disconnect`. |
| Moonraker | print | **Klipper kept printing.** Afterwards `print_stats` still showed `printing`. The history job was closed as `server_exit`, and no new job followed. |
| Klipper + Moonraker | print | `standby`. The job was closed as `klippy_disconnect`. |

What persists across all of these: uploaded files, size, `modified`, the
metadata `uuid`, history rows (Moonraker's database is a volume), and the
orphaned temp files. A Klipper restart does **not** keep `print_stats` or
`pause_resume`.

## Gate H: Capability detection

| | `moonraker` | `moonraker-multi` | `variant no-bed` | Real host |
|---|---|---|---|---|
| `file_manager`, `history`, `job_state`, `webcam` in `server.info.components` | yes | yes | yes | yes |
| `virtual_sdcard`, `pause_resume`, `print_stats`, `idle_timeout`, `webhooks` | yes | yes | yes | yes |
| `heater_bed` | yes | yes | **no** | yes |
| Objects that start with `extruder` | `extruder` | `extruder`…`extruder3` | `extruder` | `extruder`…`extruder3`, **and `extruder_offset_calibration`** |
| `toolhead.extruder` | `extruder` | `extruder` | `extruder` | `extruder1` |
| `server.webcams.list` | `[]` | `[]` | `[]` | 2 entries (`webrtc-camerastreamer` from config, `mjpegstreamer-adaptive` from the database) |
| `failed_components` | `[]` | `[]` | `[]` | `[]` |

- A prefix match on `extruder` counts **five** tools on the real host. Tool
  detection must match `^extruder\d*$` (or cross-check
  `configfile.settings`, which lists exactly `extruder`…`extruder3`).
- The real host also has components the simulator lacks: `timelapse`,
  `octoprint_compat`, `snapmakercloud`, `mqtt`, `zeroconf`, and others.
- One real webcam's `stream_url`/`snapshot_url` is an **absolute URL that
  embeds the printer's LAN address**. The other is relative. Scrub these
  before they become fixtures.
- The simulator has no webcam, so the simulator alone cannot show the
  camera capability as `supported`.

## Gate I: Clean start state

- **Simulator:** after `SMALL` completed, `print_stats.state` was
  `complete` in every sample, taken about once a second for 60 s. It was still `complete`
  after a Moonraker restart, and after pause, resume and cancel calls
  (which did nothing). **`printer.print.start` from `complete` and from
  `cancelled` both answered `ok` and started the print.** Each created a
  history job, except in the short-file case in Gate F. A Klipper restart
  clears `complete` to `standby`.
- **Real host:** `print_stats.state` was `complete` for a job whose
  `end_time` was about 68 hours earlier. `idle_timeout.state` was `Idle`,
  and `virtual_sdcard.file_path` was `null` with `progress` 1.0.
- During a Klipper shutdown, `print_stats` still reported the last
  `complete`. Readiness must come from `webhooks.state` / `klippy_state`,
  not `print_stats`.

## Decisions for the spec

Each item is a recommendation with its evidence. Task 4 decides.

1. **Checksum.** Always send `checksum=<gcode_sha256>` (lower-case hex).
   The simulator verifies it: a mismatch is a 422 and leaves nothing
   behind. Whether Snapmaker's fork (API 1.4.0) verifies it is **not
   known**: that needs a write. Moonraker's parser ignores fields it does
   not register, so a host without support would return 201 without
   checking. So a 201 proves the upload was applied, and proves its bytes
   only on a host whose checksum support has been verified. The spec must
   say whether P6 treats checksum support as verified only for the
   simulator's Moonraker version, or also for older versions.
2. **Download-and-hash rule.** No Moonraker endpoint exposes a content hash
   on either tier, and `size` plus `modified` plus metadata `uuid` is not
   identity. `locate` therefore means all of these:
   - present at `host_path`;
   - `size == gcode_size`;
   - a streamed SHA-256 of `GET /server/files/gcodes/<host_path>` equals
     `gcode_sha256`.
   Size alone never proves it. The method works (20 MB in ≤0.03 s on
   loopback), but **LAN cost is unmeasured**. The spec should bound it with
   a timeout, not a size limit. A download failure or timeout is
   inconclusive, and the row stays `uncertain`.
3. **Definitive and indeterminate responses.**

   | Operation | Definitive success | Definitive failure (not applied) | Indeterminate (`uncertain`) |
   |---|---|---|---|
   | upload | 201 | 401, 403 (file loaded), 422 (checksum), 400 (form) | no response, reset, timeout, 5xx |
   | start | 200 `ok` | 400 (`SD busy`, `Unable to open file`, shutdown, startup), 503 `Klippy Host not connected`, 401 | no response, reset, timeout, and 503 `Klippy Disconnected` (Klipper went away while the start waited; see item 4) |
   | pause / resume / cancel | **none by response alone.** 200 `ok` also comes back when nothing happened. It becomes success only when the observed state matches. | 400, 503 `Klippy Host not connected`, 401 | no response, reset, timeout, 503 `Klippy Disconnected`, or `ok` without the expected state |

   - Classify by status code, and for 503 also by message. `Klippy Host
     not connected` means Moonraker never forwarded the command. `Klippy
     Disconnected` means Klipper went away while the command was queued.
     That was seen only for a queued start (Gate E), and it only proves the
     command is no longer pending.
   - Over HTTP, a Klipper error with a multi-line message arrives as
     `"message": "Unknown"`, while WebSocket JSON-RPC carries the full
     text. Prefer the WebSocket for control if the spec
     wants the message.
   - Never surface raw error bodies. They carry tracebacks with host file
     paths.
4. **D5 "proved not applied" for start is unsafe as written.** A start
   whose response was lost, or that timed out, **can still be queued in
   Klipper and run later**. It ran 36.5 s after dispatch here. It can then
   leave no history job and no visible `print_stats` change. So
   "host standby with no such history job" does not prove the start was not
   applied. Recommended rule for a start with an indeterminate response:
   - **Proved applied:** a history job for `host_path` that passes item 5,
     or `print_stats` `printing`/`paused` with `filename == host_path`.
     A job for `host_path` above the mark whose status is
     `klippy_disconnect`, `klippy_shutdown` or `server_exit` also proves the
     start ran. Record it as **applied (interrupted)**, not as not applied.
   - **No longer pending:** a Klipper restart after dispatch. Evidence is a
     503 `Klippy Disconnected` answer to the start itself, a non-ready
     `klippy_state` observed after dispatch, or `print_stats` reset to
     `standby` with an empty filename. In the Gate E test, a queued start
     was dropped by the restart, not run. A restart only ends the waiting,
     though. A short start could already have run before the restart
     without leaving a job or a `print_stats` trace (Gate F case 1). So a
     restart does **not** prove the start was not applied.
   - **Otherwise the outcome stays unknown.** The row stays `uncertain`
     until one of the "proved applied" signals is seen, or the operator
     abandons it (D8). There is no "proved not applied" path for an
     indeterminate start, apart from a definitive error response (item 3).
   - Treating `idle_timeout.state == "Idle"` as "nothing queued" was
     considered and **rejected**. `Idle` depends on toolhead activity. After
     a restart with no motion, the state was `Idle` immediately.
   - Also give the start request a long client timeout. The queued start
     answered `ok` after 15.8 s.
4a. **The same late-apply hazard applies to pause, resume and cancel.** A
   cancel was seen queued behind a start and taking effect later (Gate E).
   For a pause, resume or cancel with an indeterminate response, "no effect
   observed" is **not** proof it was not applied while G-code may still be
   queued. It is proved applied when the observed state matches the verb.
   The exits are the same as for start: a Klipper restart means it is no
   longer pending (the outcome can still be unknown), and the operator can
   abandon it.
5. **Skew tolerance.** The measured offset is 0 on the simulator and within
   −0.08…+0.01 s on the real host. Moonraker stamps `start_time` about
   0.05 s after the transition (up to one status batch). Recommendation:
   - Record the newest history `job_id` before dispatch, as a high-water
     mark. That is clock-free.
   - Accept a job for `host_path` as proof of the start only when its
     `job_id` is above the mark **and** `start_time ≥ dispatched_at − 30 s`.
   - D9 means the printer was idle at dispatch, so an earlier job for the
     same path would have had to start and finish within 30 s before
     dispatch. A tolerance that is too small only fails safe (stays
     `uncertain`).
   - If the mark could not be read, the spec decides whether the time rule
     alone is enough.
6. **`print_stats` alone does not prove a start.**
   - `filename == host_path` with `complete` is also what the *previous*
     print of the same Slice Revision leaves behind, and a start that ran
     but finished within one status batch looks the same (Gate F case 1).
     Require the history proof from item 5, or `printing`/`paused` with
     that filename.
   - Treat a history job's `status` as a hint only. It can read
     `completed` for a file that did not run, or `server_exit` for a print
     that continued (Gate F cases 2 and 3).
7. **Partial-file handling.**
   - An interrupted upload never leaves a partial file at `host_path`. It
     leaves either the complete file (response lost) or nothing (body
     cut).
   - "Absent at `host_path`" right after the client gave up does **not**
     prove the upload was not applied. In the client-timeout runs (Gate D)
     a file absent at the first check appeared 7.3 s after the client gave
     up, because a buffering hop delivered the body late. Recommended rule:
     absent proves not applied only when either of these holds:
     - the server has provably finished with the request, for example
       because Moonraker restarted after dispatch. A restart mid-upload left
       nothing at `host_path` (Gate G). How to detect that restart is for
       the spec.
     - `host_path` is still absent after a **settle period of at least 60 s
       after the client gave up**. The latest arrival seen was 7.3 s after
       the client gave up, with an injected 10 s delay, and nothing arrived
       between 10.3 s and 48.5 s after dispatch in any run. 60 s is a margin
       picked on that evidence, not a proven bound. A host behind a slower
       buffering hop could exceed it.
     Until then the row stays `uncertain`.
   - A file at `host_path` whose size or hash differs is not ours. Classify
     it as not applied, and never delete or overwrite it automatically.
   - Orphaned `.mru` temp files build up in Moonraker's temp directory, and
     farm3d cannot see or remove them. The spec should accept that and not
     promise cleanup.
   - Re-upload stays operator-initiated (D4). Moonraker overwrites an
     existing path with 201, but refuses with 403 while that file is loaded.
8. **Start from Paused must stay blocked.** Klipper accepts it and restarts
   the file from byte 0 with `is_paused` still set. The D9 guard (re-read
   the state, `START_NOT_ALLOWED`) is the only protection.
9. **Pause from an idle printer sets `is_paused`.** The P6 UI should offer
   pause only while printing, and resume only while paused. Readiness
   should not trust `pause_resume.is_paused` without `print_stats`.
10. **Auth.** Send `X-Api-Key` on every HTTP request and on the WebSocket
    upgrade. An unauthorized WebSocket still upgrades (101) and fails per
    call with `-32602 Unauthorized`, so the capability probe must make a
    call, not just open the socket.
11. **Tools.** Match `^extruder\d*$`. The real host has
    `extruder_offset_calibration`.

## Limits

- One simulator version (Moonraker v0.11.0-1, Klipper v0.13.0-770). The
  real host runs a vendor fork at an older API level. None of its write
  behavior (checksum, 422, 403, queuing) was observed.
- Upload and download timings are loopback only.
- Each mid-body and restart case ran once per size. The control matrix and
  Gate F ran 2 and 6 times.
- The `print_stats` sequence in Gate F was sampled every 50 ms through the
  direct port. States shorter than that were not seen.
- Gate E has no row for the `print_stats` `error` state. Reaching it needs a
  failing print, and none was run.
