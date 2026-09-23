import { createEffect, createMemo, createResource, createSignal, For, on, Show } from "solid-js";
import { Button, Chip, Select, TextField } from "../design-system";
import { discoverPrinters } from "../printers/printer-store";
import type {
  ConnectionSubmission,
  PrinterProfile,
  ProbeResult,
  ReportedCapabilities,
} from "../printers/types";
import styles from "./ConnectionFields.module.css";

/** Only what this build can actually speak. Phase 3 appends OctoPrint and
 *  ElegooLink here as each adapter lands — listing them now as disabled
 *  entries would need a prop `Select` does not have (verified: `SelectProps`
 *  exposes no `optionDisabled`), and offering a kind that errors on save is
 *  worse than not offering it. */
export const KINDS = [{ value: "moonraker", label: "Moonraker (Klipper)" }];

/** Catalog `suggestedHostType`s name every host the source catalog knows
 *  (PrusaLink, OctoPrint, ...), not just the adapters this build ships; an
 *  unsupported one would leave the Kind Select blank and save a Connection
 *  that can only be Setup incomplete. */
export function supportedKind(kind: string | null | undefined): string | undefined {
  return KINDS.some((k) => k.value === kind) ? kind ?? undefined : undefined;
}

const DEFAULT_PORTS: Record<string, number> = { moonraker: 7125, octoprint: 80 };

/** Millimetre-scale float noise is not a disagreement worth a warning. */
const TOLERANCE_MM = 1;

/**
 * Compares what the host reports against the catalog Profile. A mismatch
 * usually means the wrong catalog variant was picked in the setup flow —
 * worth surfacing, since phase 3 will slice against these numbers.
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

export interface ConnectionDraft {
  kind: string;
  host: string;
  port: number;
  useTls: boolean;
  credential: string;
}

/** Whether `next` describes a materially different submission than
 *  `previous` — i.e. whether a probe run against `previous` is now stale.
 *  kind/host/port/useTls always matter. `credential` only counts as a
 *  change when it becomes a *new, non-blank* value (a freshly typed key) —
 *  a blank credential means "unset"/"leave the stored key alone" (see
 *  `toSubmission`), and `PrinterConnectionPanel` resets the field back to
 *  `""` after every Save (success or failure) purely to stop echoing a
 *  typed secret, not because the connection changed. Without this
 *  exception, that post-Save reset would itself invalidate a probe that's
 *  still perfectly valid. Shared by `ConnectionFields`' own probe state and
 *  `PrinterSetupWizard`'s `lastProbe`, so the two can't drift on this
 *  rule. */
export function connectionDraftChanged(previous: ConnectionDraft, next: ConnectionDraft): boolean {
  if (previous.kind !== next.kind) return true;
  if (previous.host !== next.host) return true;
  if (previous.port !== next.port) return true;
  if (previous.useTls !== next.useTls) return true;
  if (next.credential !== "" && next.credential !== previous.credential) return true;
  return false;
}

/** `create`: a brand-new Printer with no stored credential yet — a blank
 *  credential means "no credential at all", so it is omitted from the
 *  submission. `edit`: a Printer that may already have a stored credential
 *  — a blank credential means "leave the stored key alone" (the UI never
 *  echoes a stored secret back), so it is *also* omitted, just for a
 *  different reason. Either way, a non-blank credential is trimmed and
 *  sent. */
export function toSubmission(
  draft: ConnectionDraft,
  _mode: "create" | "edit",
): ConnectionSubmission {
  const credential = draft.credential.trim();
  return {
    kind: draft.kind,
    host: draft.host.trim(),
    port: draft.port,
    useTls: draft.useTls,
    ...(credential === "" ? {} : { credential }),
  };
}

export interface ConnectionFieldsProps {
  value: ConnectionDraft;
  onChange: (next: ConnectionDraft) => void;
  /** A default `kind` to seed once, e.g. from the catalog variant's
   *  `suggestedHostType` — applied only until the user picks a Kind
   *  themselves, so a later Identify-step change doesn't clobber a
   *  deliberate choice. */
  suggestedKind?: string;
  /** wizard: `probeCandidate`; dock: `testConnection(id, …)`. */
  onTest: () => Promise<ProbeResult>;
  /** Enables the capability-mismatch list; omitted where there is no
   *  catalog Profile yet to compare against. */
  profile?: PrinterProfile;
  /** e.g. "leave blank to keep the stored key" in dock/edit mode. Left
   *  undefined in create mode, where there is nothing stored to keep. */
  credentialHint?: string;
}

/** The kind/host/port/credential fields, discovery list, Test button,
 *  ProbeResult chip, and capability-mismatch list shared by
 *  `PrinterConnectionPanel` (existing-Printer/edit mode) and
 *  `PrinterSetupWizard` (candidate/create mode). Field *values* are fully
 *  controlled by the caller via `value`/`onChange`; only the Test-probe
 *  result and the discovery list are local to this component. */
export function ConnectionFields(props: ConnectionFieldsProps) {
  const [kindTouched, setKindTouched] = createSignal(false);
  const [probe, setProbe] = createSignal<ProbeResult | null>(null);
  const [probeError, setProbeError] = createSignal<string | null>(null);
  const [testing, setTesting] = createSignal(false);

  const [discovered, { refetch: rediscover }] = createResource(discoverPrinters);

  // Seeds `kind` from the suggested host type once, without clobbering a
  // deliberate pick the user already made (e.g. after opening the Kind
  // Select themselves).
  createEffect(() => {
    const suggested = supportedKind(props.suggestedKind);
    if (!suggested || kindTouched() || props.value.kind === suggested) return;
    props.onChange({ ...props.value, kind: suggested, port: DEFAULT_PORTS[suggested] ?? props.value.port });
  });

  // A verified `probe`/`probeError` describes the *specific* submission it
  // was run against — a materially different draft afterward (per
  // `connectionDraftChanged`) invalidates it, or the chip/mismatch list
  // would keep showing a stale "online" result (or error) for a config the
  // user has since changed. `previous === undefined` skips the run `on()`
  // always does at mount, where there's nothing stale to clear yet.
  createEffect(
    on(
      () => props.value,
      (current, previous) => {
        if (previous === undefined || !connectionDraftChanged(previous, current)) return;
        setProbe(null);
        setProbeError(null);
      },
    ),
  );

  const mismatches = createMemo(() => {
    const result = probe();
    return result && props.profile ? buildMismatches(props.profile, result.reported) : [];
  });

  async function onTest() {
    setTesting(true);
    setProbeError(null);
    try {
      setProbe(await props.onTest());
    } catch (e) {
      setProbe(null);
      setProbeError(String(e));
    } finally {
      setTesting(false);
    }
  }

  return (
    <div class={styles.fields}>
      <Select
        label="Kind"
        options={KINDS}
        optionValue={(k: (typeof KINDS)[number]) => k.value}
        optionLabel={(k: (typeof KINDS)[number]) => k.label}
        value={KINDS.find((k) => k.value === props.value.kind)}
        onChange={(k: (typeof KINDS)[number]) => {
          setKindTouched(true);
          props.onChange({ ...props.value, kind: k.value, port: DEFAULT_PORTS[k.value] ?? props.value.port });
        }}
      />

      <TextField
        label="Host"
        value={props.value.host}
        onChange={(host) => props.onChange({ ...props.value, host })}
        placeholder="voron.local"
      />
      <TextField
        label="Port"
        value={String(props.value.port)}
        onChange={(v) =>
          props.onChange({
            ...props.value,
            port: Number(v) || DEFAULT_PORTS[props.value.kind] || 7125,
          })
        }
      />
      <TextField
        label="API key"
        type="password"
        value={props.value.credential}
        onChange={(credential) => props.onChange({ ...props.value, credential })}
        placeholder={props.credentialHint ?? "Optional"}
      />

      {/* Discovery is an accelerator: the fields above work with it empty. */}
      <div class={styles.discovery}>
        <div class={styles.discoveryHeader}>
          <span class={styles.sectionTitle}>Discovered on this network</span>
          <Button variant="ghost" onClick={() => void rediscover()}>
            Rescan
          </Button>
        </div>
        <Show
          when={!discovered.error}
          fallback={<p class={styles.warn}>Discovery failed — enter the host above.</p>}
        >
          <Show
            when={(discovered() ?? []).length > 0}
            fallback={<p class={styles.note}>Nothing found — enter the host above.</p>}
          >
            <For each={discovered()}>
              {(found) => (
                <Button
                  variant="ghost"
                  onClick={() => {
                    setKindTouched(true);
                    props.onChange({ ...props.value, kind: found.kind, host: found.host, port: found.port });
                  }}
                >
                  {found.name} — {found.host}:{found.port}
                </Button>
              )}
            </For>
          </Show>
        </Show>
      </div>

      <div class={styles.testRow}>
        <Button variant="secondary" disabled={testing()} onClick={() => void onTest()}>
          {testing() ? "Testing…" : "Test connection"}
        </Button>
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
