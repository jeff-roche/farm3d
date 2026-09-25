# ElegooLink (SDCP) fake

Elegoo publishes no simulator, so ElegooLink has no container here.
Instead, [`src-tauri/tests/sim/elegoolink.rs`](../../src-tauri/tests/sim/elegoolink.rs)
runs a fake SDCP server inside the test process. Its tests are hermetic
and run in the normal Rust suite (`sim_elegoolink.rs`).

## The rule

The fake does only what a recorded real-hardware capture shows a real
printer doing. Nothing may come from desk research, public write-ups, or
guesses about the protocol (ADR-0012). It reads #8's redacted captures
from `docs/superpowers/baselines/evidence/a0-3-elegoolink/` and derives
its behavior from them when it starts:

| Behavior | Where it comes from |
| --- | --- |
| Answers Cmd 0, 1, 258, 320, and 321 with the recorded ack (the caller's `RequestID` put in) and the recorded follow-up frames | Requests and replies in the idle captures (S3, S3b, S3c, S3d, S4) |
| `Status` only in reply to Cmd 0; nothing is ever pushed | S3 and S3b: no unprompted frames |
| No reply to a text `ping` | S3: eleven pings, no pong |
| Drops a silent client without a close frame (code 1006) after about 61 s | S4: last client frame at 4.4 s, drop at 65.8 s |
| Hex-encoded `PrintInfo` keys, such as `54 6F 74 61 6C 45 78 74 72 75 73 69 6F 6E 00` | Replayed byte for byte from the recorded `Status` frames |
| Answers UDP `M99999` with the recorded discovery reply | S2 |

Any other request gets no answer, and `FakeSdcp::unmodelled()` lists it.

## Status

- **Idle and read-only behavior:** implemented, and derived from #8's
  captures. It activates when those captures reach `main`. Until then the
  tests print PENDING. To run them against a local copy, set
  `FARM3D_SIM_ELEGOOLINK_CAPTURES` to a directory of capture files.
- **Command behavior (upload, start, pause, resume, cancel, camera):**
  **pending.** The owner accepts this fake as the evidence source for
  farm3d's ElegooLink commands, so a Centauri Carbon never receives writes
  from automated tests. Each command may be modelled only from #8's
  passive captures of another client sending it (runbook scenarios S7,
  S7b, and S10p). None exist yet, so every command is unanswered.

The fake is never evidence for #8's own monitoring gates, which need real
hardware.
