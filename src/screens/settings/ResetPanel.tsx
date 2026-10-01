import { createSignal, For, onMount, Show, type JSX } from "solid-js";
import { Button, Checkbox, RadioGroup, TypedConfirmDialog } from "../../design-system";
import { diagnostics, loadResetPreview, resetFarm } from "../../diagnostics/diagnostics-store";
import { formatBytes, resetClassLabel, resetEffectLabel, resetSummaryText } from "../../diagnostics/presentation";
import type { ResetMediaScope, ResetPreview, ResetRequest, ResetTier } from "../../diagnostics/types";
import type { ResetWarning } from "../../generated/contracts/domain/ResetWarning";
import { loadSettings, settings } from "../../settings/settings-store";
import { errorText } from "./error-text";
import styles from "./Settings.module.css";
import portability from "./Portability.module.css";

const plural = (count: number, one: string, many: string): string => `${count} ${count === 1 ? one : many}`;

function warningText(warning: ResetWarning): string {
  if (warning.kind === "credentialStoreUnavailable") {
    return "The credential store is unavailable, so stored credentials may not be deleted.";
  }
  const parts = [
    warning.activeJobs > 0 ? plural(warning.activeJobs, "active Job", "active Jobs") : null,
    warning.hostOperations > 0 ? plural(warning.hostOperations, "Printer command in flight", "Printer commands in flight") : null,
    warning.sliceOperations > 0 ? plural(warning.sliceOperations, "slice in progress", "slices in progress") : null,
  ].filter(Boolean);
  return `Work is still active (${parts.join(", ")}). A reset interrupts it.`;
}

function classLines(preview: ResetPreview | undefined): string[] {
  return (preview?.classes ?? []).map((entry) => {
    const size = [entry.count !== null ? String(entry.count) : null, entry.bytes !== null ? formatBytes(entry.bytes) : null]
      .filter(Boolean).join(", ");
    return `${resetEffectLabel(entry.effect)}: ${resetClassLabel(entry.class)}${size ? ` (${size})` : ""}`;
  });
}

interface TierProps {
  tier: ResetTier;
  title: string;
  description: string;
  buttonLabel: string;
  confirmLabel: string;
  restarts?: boolean;
  /** `null` while the tier can't run yet (e.g. settings not loaded). */
  request: () => ResetRequest | null;
  extraConsequences?: () => string[];
  children?: JSX.Element;
}

function Tier(props: TierProps) {
  const [open, setOpen] = createSignal(false);
  const [pending, setPending] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [result, setResult] = createSignal<string | null>(null);
  const preview = () => diagnostics.resetPreview(props.tier);

  async function confirm() {
    const request = props.request();
    const phrase = preview()?.phrase;
    if (!request || !phrase) return;
    setPending(true);
    setError(null);
    try {
      setResult(resetSummaryText(await resetFarm(request, phrase)));
      setOpen(false);
    } catch (failure) {
      setError(errorText(failure));
    }
    setPending(false);
  }

  return (
    <section class={`${styles.section}`} aria-label={props.title}>
      <span class={styles.sectionTitle}>{props.title}</span>
      <p class={styles.note}>{props.description}</p>
      <Show when={preview()} fallback={<p class={styles.note}>Checking what this would change…</p>}>
        {(current) => (
          <>
            <ul class={portability.rows} aria-label={`${props.title}: what changes`}>
              <For each={current().classes}>
                {(entry) => (
                  <li class={portability.row}>
                    <span class={portability.grow}>{resetClassLabel(entry.class)}</span>
                    <span class={portability.effect}>{resetEffectLabel(entry.effect)}</span>
                    <span class={`${portability.meta} ${portability.numeric}`}>
                      {[entry.count !== null ? String(entry.count) : null, entry.bytes !== null ? formatBytes(entry.bytes) : null].filter(Boolean).join(", ")}
                    </span>
                  </li>
                )}
              </For>
            </ul>
            <For each={current().warnings}>{(warning) => <p class={portability.warning}>{warningText(warning)}</p>}</For>
          </>
        )}
      </Show>
      {props.children}
      <div class={styles.actions}>
        <Button variant="danger" disabled={!preview() || !props.request()} onClick={() => { setError(null); setOpen(true); }}>
          {props.buttonLabel}
        </Button>
      </div>
      <Show when={result()}>
        {(text) => <p class={styles.status} role="status">{text()}</p>}
      </Show>
      <TypedConfirmDialog
        open={open()}
        onOpenChange={(next) => { if (!pending()) setOpen(next); }}
        title={props.title}
        consequences={[
          ...classLines(preview()),
          ...(preview()?.warnings.map(warningText) ?? []),
          ...(props.extraConsequences?.() ?? []),
          ...(props.restarts ? ["farm3d will restart to finish the reset."] : []),
        ]}
        phrase={preview()?.phrase ?? ""}
        confirmLabel={props.confirmLabel}
        pending={pending()}
        error={error()}
        onConfirm={() => void confirm()}
      />
    </section>
  );
}

/** The tiered reset (spec D8): settings, camera images, and the Farm. Rust
 *  decides what each tier touches; this shows its `reset_preview`. */
export function ResetPanel() {
  const [scope, setScope] = createSignal<ResetMediaScope>("unpinned");
  const [safetyBackup, setSafetyBackup] = createSignal(true);
  const [deleteSafety, setDeleteSafety] = createSignal(false);

  onMount(() => {
    void loadSettings().catch(() => {});
    for (const tier of ["settings", "cameraMedia", "farm"] as const) void loadResetPreview(tier).catch(() => {});
  });

  return (
    <div class={styles.category}>
      <h4 class={portability.subheading}>Reset</h4>
      <p class={styles.note}>Each reset lists exactly what it changes and asks you to type a phrase first.</p>

      <Tier
        tier="settings"
        title="Reset settings"
        description="Puts every setting back to its default. Printers, Jobs, and your library are kept."
        buttonLabel="Reset settings…"
        confirmLabel="Reset settings"
        request={() => {
          const revision = settings()?.revision;
          return revision === undefined ? null : { tier: "settings", expectedRevision: revision };
        }}
      />

      <Tier
        tier="cameraMedia"
        title="Reset camera images"
        description="Removes camera images from disk. Their records stay, marked as removed by a reset."
        buttonLabel="Reset camera images…"
        confirmLabel="Reset camera images"
        request={() => ({ tier: "cameraMedia", scope: scope() })}
        extraConsequences={() => (scope() === "all" ? ["Pinned camera images are removed too."] : [])}
      >
        <RadioGroup
          label="Scope"
          value={scope()}
          onChange={(value) => setScope(value as ResetMediaScope)}
          options={[
            { value: "unpinned", label: "Unpinned camera images only" },
            { value: "all", label: "All camera images" },
          ]}
        />
        <Show when={scope() === "all"}>
          <p class={portability.warning}>
            Choosing All camera images also removes pinned evidence images, even though the list above shows them as kept.
          </p>
        </Show>
      </Tier>

      <Tier
        tier="farm"
        title="Reset the Farm"
        description="Returns farm3d to a fresh install: your Printers, Spools, Models, Jobs, and settings are removed."
        buttonLabel="Reset the Farm…"
        confirmLabel="Reset and restart"
        restarts
        request={() => ({ tier: "farm", safetyBackup: safetyBackup(), deleteSafetyBackups: deleteSafety() })}
        extraConsequences={() => [
          safetyBackup() ? "A safety backup is written first, so you can restore." : "No safety backup is written; this can't be undone.",
          ...(deleteSafety() ? ["Existing safety backups are deleted too."] : []),
        ]}
      >
        <div class={styles.radioList}>
          <Checkbox checked={safetyBackup()} onChange={setSafetyBackup}>Write a safety backup first</Checkbox>
          <Checkbox checked={deleteSafety()} onChange={setDeleteSafety}>Also delete safety backups</Checkbox>
        </div>
      </Tier>
    </div>
  );
}
