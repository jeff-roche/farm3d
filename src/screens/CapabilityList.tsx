import { createSignal, For, onMount, Show } from "solid-js";
import { capabilities, loadCapabilities } from "../host-ops/capabilities-store";
import { CAPABILITY_KEYS, capabilityName, capabilityStateLabel, evidenceTierLabel } from "../host-ops/presentation";
import type { CapabilityKey, PrinterCapabilities } from "../host-ops/types";
import { HostOperationAlert } from "./HostOperationAlert";
import { SeverityLabel } from "./SeverityLabel";
import styles from "./CapabilityList.module.css";

export interface CapabilityListProps {
  printerId: string;
}

/** The Setup tab's read-only capability list (spec "Components"): each
 *  capability's label, and either its evidence tier or why it isn't
 *  available. Unsupported is neutral, never styled as a failure. */
export function CapabilityList(props: CapabilityListProps) {
  const [loadError, setLoadError] = createSignal<unknown>(null);
  const held = () => capabilities.forPrinter(props.printerId);

  const load = () => {
    setLoadError(null);
    loadCapabilities(props.printerId).catch(setLoadError);
  };
  onMount(() => {
    if (!held()) load();
  });

  return (
    <section class={styles.section} aria-labelledby={`capabilities-${props.printerId}`}>
      <h3 id={`capabilities-${props.printerId}`} class={styles.title}>Capabilities</h3>
      <Show
        when={held()}
        fallback={
          <Show
            when={loadError()}
            fallback={<p class={styles.note}>Checking what this printer supports…</p>}
          >
            {(error) => (
              <HostOperationAlert error={error()} fallback="farm3d couldn't read what this printer supports." onRetry={load} />
            )}
          </Show>
        }
      >
        {(record) => (
          <ul class={styles.list} aria-label="Capabilities">
            <For each={CAPABILITY_KEYS}>{(key) => <CapabilityRow record={record()} capability={key} />}</For>
          </ul>
        )}
      </Show>
    </section>
  );
}

function CapabilityRow(props: { record: PrinterCapabilities; capability: CapabilityKey }) {
  const state = () => props.record.capabilities[props.capability];
  const label = () => capabilityStateLabel(state(), props.record.adapterKind);
  const aside = () => {
    const current = state();
    return current.status === "supported" ? evidenceTierLabel(current.evidence.tier) : current.detail;
  };
  return (
    <li class={styles.row}>
      <span class={styles.name}>{capabilityName(props.capability)}</span>
      <SeverityLabel severity={label().severity} text={label().text} />
      <span class={styles.aside}>{aside()}</span>
    </li>
  );
}
