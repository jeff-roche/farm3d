# A safe-by-construction diagnostics log and egress-scanned bundles

**Status:** Draft, pending controller review, 2026-09-29, with the P9
spec.

## Context

The umbrella spec asks for a selectable diagnostics export that "redacts
credentials" and includes "versions, adapter health, relevant logs, and
configuration summaries" (issue #19). Its reader is a third party: a
support thread or a public issue.

What farm3d holds that must never reach that reader:

- credential values, and the refs that name them;
- Printer hosts, IPs, and ports, in Connections and in Host Operation
  endpoints;
- camera snapshot URLs, which can carry a query token;
- absolute paths (linked Models, the Slicer runtime) and so the
  operator's username;
- user-authored names and notes (Printers, Spools, tares, Models,
  Projects, Locations, Incident notes);
- host-supplied strings (job and file names).

What exists on `main` today:

- There is no log file. About twenty `eprintln!` sites write to stderr,
  and several print `{error:?}`. The Moonraker WebSocket errors keep
  `e.to_string()`, which can hold the URL. Those strings stay internal
  only because nothing collects them yet.
- There is no general secret type. Secrets are kept out of serializable
  structs, carried in `Zeroizing` buffers, and wrapped by hand-written
  redacting `Debug` impls.
- Each phase's tests have their own seeded-secret corpus and scanner.

| Option | For | Against |
|---|---|---|
| A. A general logging crate (`log`/`tracing`) with free-text messages, redacted by regex at export | Familiar; any value can be logged | Redaction by pattern is guesswork: a host, a name, or a new URL shape passes a regex that didn't expect it. Every future log call is a possible leak, and review has to catch it |
| B. Free-text log, and the export drops the log entirely | Simple and safe | Loses the most useful support evidence (what failed, when, how often) |
| C. A log that accepts only typed, closed-vocabulary fields, collectors that emit only typed facts and pseudonyms, and a final egress scan against the live secret corpus | Free text can't compile into a log line; the bundle's only strings come from enums, codes, versions, timestamps, and pseudonyms; the scan is a second, independent check with the actual secrets | Log calls are more verbose; errors log their variant, not their message, so some detail is lost; the scan only catches values it knows |

## Decision

**Option C.**

- **The log is typed.** `f3d_log!(level, "domain.code", key = value, …)`
  accepts only values of a sealed `LogSafe` trait: typed id wrappers
  (`LogId`), `ErrorCode` and opted-in domain enums, error types by variant
  name only, integers, booleans, durations, timestamps, and `Pseudonym`.
  `String`, `&str`, paths, URLs, and every error's `Display` are not
  `LogSafe`. Every `eprintln!` moves to it, and a test forbids new ones.
- **The log is bounded.** JSON lines in `<log_root>/farm3d.log`, rotated
  at 2 MiB, five files kept, written by a background thread that drops
  (and counts) rather than blocks.
- **Ids are kept apart.** Each line splits `ids` from `fields`, so the
  bundle can replace every id with a pseudonym.
- **The bundle is pseudonymized by construction.** Each section's
  collector emits typed facts only. Printers and other records appear as
  per-bundle ordinal pseudonyms (`printer-3`) drawn from a fresh random
  salt, stable within a bundle and different between bundles. Host
  software versions are kept only as short version tokens.
- **Then it is egress-scanned.** Before any file is written, every entry
  is scanned against a corpus read from the live Farm: every credential
  value behind every ref, every host, every camera URL and its query
  values, the home directory, and every user-authored name longer than
  three characters, in exact, case-changed, and percent-encoded forms. A
  hit refuses the whole bundle with `DIAGNOSTICS_REDACTION_FAILED`, names
  only the section, and writes nothing.

The P9 spec
(`docs/superpowers/specs/2026-09-29-p9-history-settings-backup-diagnostics-design.md`)
holds the line format, the `LogSafe` implementors, the section contents,
and the scan rules.

## Consequences

- Free text can't reach the log or a bundle through a new call site; a
  reviewer checks types, not strings.
- Log lines carry less detail than an error message would. A support case
  gets the code, the error kind, the ids (as pseudonyms), and the counts,
  not the host's own words.
- Log files on disk hold raw ids; they never leave the machine except
  through a bundle, which rewrites them.
- The egress scan is defense in depth. It can't catch a transformation it
  has no variant for (a partial or re-encoded value); the typed log and
  collectors are the primary control.
- A Printer named after a word the bundle's schema uses can't block the
  export, because names are matched only against free string values;
  secrets and hosts are matched against every byte.
- Future sections must be typed collectors, and any new string field
  needs a reason it can't carry a protected value.
