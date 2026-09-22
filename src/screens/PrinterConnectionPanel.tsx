import { createMemo, createResource, createSignal, For, Show } from "solid-js";
import { Button, Chip, Select, TextField } from "../design-system";
import {
  clearConnection,
  credentialStoreInfo,
  discoverPrinters,
  reportError,
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
    credential: apiKey() === "" ? undefined : apiKey(),
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
      setApiKey("");
      setTesting(false);
    }
  }

  async function onSave() {
    try {
      await setConnection(props.printer.id, submission());
    } catch (e) {
      // `setConnection` now rejects (Ruling R2) so a caller that renders the
      // failure inline can offer "Save anyway" (`acceptUnverified`). This
      // panel doesn't do that yet (Task 11), so it falls back to the same
      // store error banner `setConnection` used to populate itself.
      reportError(e);
    } finally {
      setApiKey("");
    }
  }

  return (
    <div class={styles.panel}>
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
            <Show when={info().reasonCode}>
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
        </Show>
      </div>

      <div class={styles.actions}>
        <Button variant="secondary" disabled={testing()} onClick={() => void onTest()}>
          {testing() ? "Testing…" : "Test connection"}
        </Button>
        <Button variant="primary" onClick={() => void onSave()}>
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
