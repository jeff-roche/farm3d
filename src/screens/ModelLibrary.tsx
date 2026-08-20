import { createMemo, createSignal, For, Show } from "solid-js";
import { IconRefresh } from "@tabler/icons-solidjs";
import { Button, IconButton, Select } from "../design-system";
import { BuildPlate } from "./BuildPlate";
import styles from "./ModelLibrary.module.css";

export interface Model {
  id: string;
  name: string;
  addedAt: string;
}

export interface ModelLibraryProps {
  models: Model[];
  compatiblePrinterNames: string[];
}

export function ModelLibrary(props: ModelLibraryProps) {
  const [selectedId, setSelectedId] = createSignal<string | null>(null);
  const selected = createMemo(() => props.models.find((m) => m.id === selectedId()));

  return (
    <div class={styles.library}>
      <nav class={styles.list} aria-label="Model library">
        <div class={styles.panelHeader}>Library</div>
        <Show
          when={props.models.length > 0}
          fallback={<p class={styles.empty}>Models you add will show up here.</p>}
        >
          <ul class={styles.modelList}>
            <For each={props.models}>
              {(model) => (
                <li>
                  <button
                    class={styles.modelRow}
                    classList={{ [styles.modelRowSelected]: model.id === selectedId() }}
                    onClick={() => setSelectedId(model.id)}
                  >
                    <span class={styles.modelThumb} aria-hidden="true" />
                    <span class={styles.modelName}>{model.name}</span>
                    <span class={styles.modelAdded}>{model.addedAt}</span>
                  </button>
                </li>
              )}
            </For>
          </ul>
        </Show>
      </nav>

      <div class={styles.viewport}>
        <BuildPlate />

        <div class={styles.viewportActions}>
          <IconButton aria-label="Reset view" disabled title="Viewport controls aren't wired up yet">
            <IconRefresh size={14} />
          </IconButton>
          <Button
            variant="secondary"
            size="sm"
            disabled
            title="Library membership isn't wired up yet"
          >
            Add to Library
          </Button>
          <Button variant="primary" size="sm" disabled title="Slicing isn't wired up yet">
            Slice
          </Button>
        </div>

        <Show when={!selected()}>
          <p class={styles.viewportHint}>Select a model from the library to inspect it.</p>
        </Show>
      </div>

      <aside class={styles.detail} aria-label="Slice settings">
        <div class={styles.panelHeader}>Slice</div>
        <Show
          when={selected()}
          fallback={<p class={styles.detailEmpty}>Select a model to configure slicing.</p>}
        >
          {(model) => (
            <div class={styles.detailBody}>
              <div class={styles.detailField}>
                <span class={styles.detailLabel}>Model</span>
                <span>{model().name}</span>
              </div>
              <Select
                label="Target printer"
                options={props.compatiblePrinterNames}
                placeholder="Choose a compatible printer"
                disabled
              />
              <Button variant="primary" disabled title="Slicing isn't wired up yet">
                Slice
              </Button>
              <Button variant="secondary" disabled title="Dispatch isn't wired up yet">
                Dispatch
              </Button>
            </div>
          )}
        </Show>
      </aside>
    </div>
  );
}
