# Composable connection capabilities

**Status:** Accepted (approved by the controller, owner-delegated), 2026-09-25.

## Context

Until P6, a Connection (ADR-0002) only observed a Printer: the
`PrinterConnection` trait has `probe` and `subscribe`. P6 adds writes for
Moonraker: staging a Slice Revision on the host, starting it, pausing,
resuming, and cancelling, plus reading the host state farm3d needs to
reconcile those writes and reporting whether a camera is configured.

The adapters do not support the same things. Moonraker gets every P6
capability. OctoPrint stays status-only until its own spike. A future
ElegooLink adapter may support some commands and not others, and a given
host can lack a feature its adapter supports (a printer with no
`pause_resume`, or no camera). farm3d has to say, per Printer, which
capabilities exist and on what evidence, and "unsupported" must never
look like "failed".

The Moonraker command spike
(`docs/superpowers/baselines/2026-09-25-p6-moonraker-command-spike.md`)
also showed that a write's outcome can be unknown after a lost response,
and that a command can apply late. Writes therefore need a durable record
and a read-only way to check the host afterwards.

Three shapes were considered:

| Option | For | Against |
|---|---|---|
| A. Add methods to `PrinterConnection`, with default "unsupported" bodies | Smallest change | "Not implemented" looks the same as "the protocol can't". Every fake grows. Writes would share the supervisor's long-lived observation socket. |
| B. One `PrinterCommands` trait | Separates reads from writes | All or nothing per adapter. Host state and camera discovery are not commands. |
| C. Small capability traits, and a registry that says which ones each adapter builds | Unsupported is structural: the builder is absent. Fakes stay small. Adapters gain one capability at a time. | More types, and a registry that has to stay honest |

## Decision

**Option C.** Four capability traits live in
`connections/capabilities.rs`:

- `ArtifactStaging`: `upload`, `locate`;
- `PrintControl`: `start`, `pause`, `resume`, `cancel`;
- `HostStateQuery`: `host_facts`, `host_job_state`, `job_history`;
- `CameraDiscovery`: `cameras`.

Each adapter's `AdapterDescriptor` in the registry (`connections/adapters.rs`,
built on #27's `SUPPORTED_KINDS` and `build_connection`) carries an
optional builder for each trait, beside the existing observe builder, and
an evidence row per capability. A capability is `supported` only when its
builder exists **and** it has evidence. Evidence from the read-only
real-hardware tier can never make a write capability supported, because
real printers are never written to (ADR-0012).

`capabilities_for(printer, host_facts)` turns the descriptor, the
Printer's Connection, and the host's reported facts into one
`PrinterCapabilities` value. The UI, the commands, and P7 all ask it. A
command whose capability is unsupported returns `CAPABILITY_UNSUPPORTED`
and records nothing.

Commands build their capability objects per operation, from the
Connection config and credential. They never use the supervisor's
observation socket, which keeps observing unchanged.

Every write is a Host Operation: a row committed before the first byte
is sent, moved to `uncertain` when the outcome is unknown, and resolved
only by proof read back from the host or by the operator abandoning it.
The P6 spec (`docs/superpowers/specs/2026-09-25-p6-connection-command-capabilities-design.md`)
holds the full rules.

## Consequences

- Adding a capability to an adapter means adding a builder, a test suite
  against its fake or simulator, and an evidence row. Until the evidence
  row exists, the capability reports `notVerified`, not `supported`.
- A registry consistency test fails if a capability is `supported` with no
  builder, or has a builder with no evidence.
- The observation path (`PrinterConnection`, the supervisor) is untouched,
  so status monitoring cannot regress through P6's write code.
- Capability objects open their own connections. For Moonraker that is
  plain HTTP per operation, beside the supervisor's WebSocket.
- There are more types to keep in step: four traits, their builders, the
  evidence rows, and the `CapabilityKey` list the frontend renders.
- OctoPrint and ElegooLink can gain command capabilities later without
  touching Moonraker's, one trait at a time.
