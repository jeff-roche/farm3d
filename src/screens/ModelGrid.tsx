import { createResource, createUniqueId, For, Show } from "solid-js";
import { IconCube, IconFileCode } from "@tabler/icons-solidjs";
import { SeverityMarker } from "../design-system";
import { loadThumbnail } from "../library/library-store";
import type { LibraryView } from "../library/saved-views";
import type { ModelRecord, ProjectRecord } from "../library/types";
import { formatLabel, sourceStateMarker, storageLabel } from "./library-presentation";
import { ModelActionsMenu } from "./ModelActionsMenu";
import styles from "./ModelGrid.module.css";

export interface ModelGridProps {
  /** Already filtered and sorted by the caller. */
  models: ModelRecord[];
  projects: ProjectRecord[];
  view: LibraryView;
  selectedId: string | null;
  onSelect: (modelId: string) => void;
  onAddToProject: (modelId: string) => void;
}

/** D19 grid mode: one card per Model with its thumbnail (or format icon),
 *  name, format, and a storage/source-state badge. Cards are native
 *  buttons, so Enter and Space select them without a key handler of our
 *  own; `aria-pressed` marks the selected one. */
export function ModelGrid(props: ModelGridProps) {
  return (
    <ul class={styles.grid} aria-label="Models">
      <For each={props.models}>
        {(model) => (
          <li class={styles.cell}>
            <ModelCard
              model={model}
              selected={model.id === props.selectedId}
              onSelect={() => props.onSelect(model.id)}
            />
            <div class={styles.menu}>
              <ModelActionsMenu
                model={model}
                view={props.view}
                projects={props.projects}
                onAddToProject={props.onAddToProject}
              />
            </div>
          </li>
        )}
      </For>
    </ul>
  );
}

function ModelCard(props: { model: ModelRecord; selected: boolean; onSelect: () => void }) {
  const nameId = createUniqueId();
  const metaId = createUniqueId();
  const link = () => props.model.link;
  return (
    <button
      type="button"
      class={styles.card}
      aria-pressed={props.selected}
      aria-labelledby={nameId}
      aria-describedby={metaId}
      onClick={() => props.onSelect()}
    >
      <ModelThumbnail model={props.model} />
      <span id={nameId} class={styles.name}>{props.model.name}</span>
      <span id={metaId} class={styles.meta}>
        <span>{formatLabel(props.model.format)}</span>
        <Show
          when={link() && link()!.state !== "ok"}
          fallback={<span class={styles.badge}>{storageLabel(props.model)}</span>}
        >
          <SeverityMarker {...sourceStateMarker(link()!.state)} />
        </Show>
      </span>
    </button>
  );
}

/** The current revision's embedded thumbnail, or the format icon with its
 *  name when there is none (always for STL). */
export function ModelThumbnail(props: { model: ModelRecord }) {
  const revision = () => props.model.currentRevision;
  const [source] = createResource(
    () => (revision().hasThumbnail ? revision().id : false),
    (revisionId) => loadThumbnail(revisionId).catch(() => null),
  );
  return (
    <span class={styles.thumbnail}>
      <Show
        when={source()}
        fallback={
          <span class={styles.formatIcon} data-format-icon>
            <Show when={props.model.format === "gcode"} fallback={<IconCube size={28} aria-hidden="true" />}>
              <IconFileCode size={28} aria-hidden="true" />
            </Show>
            <span>{formatLabel(props.model.format)}</span>
          </span>
        }
      >
        {(src) => <img class={styles.image} src={src()} alt="" />}
      </Show>
    </span>
  );
}
