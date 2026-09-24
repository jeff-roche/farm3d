import { createMemo, For } from "solid-js";
import { IconDots } from "@tabler/icons-solidjs";
import { DropdownMenu } from "../design-system";
import { modelsFor, SAVED_VIEW_IDS, viewCounts, type LibraryView } from "../library/saved-views";
import type { ModelRecord, ProjectRecord } from "../library/types";
import { SAVED_VIEW_LABELS } from "./library-presentation";
import styles from "./LibrarySidebar.module.css";

export interface LibrarySidebarProps {
  view: LibraryView;
  projects: ProjectRecord[];
  models: ModelRecord[];
  now: Date;
  onSelectView: (view: LibraryView) => void;
  /** Each Project row's **Rename…** and **Delete…**; the caller opens the
   *  dialog. */
  onRenameProject: (projectId: string) => void;
  onDeleteProject: (projectId: string) => void;
}

/** D19: the saved views, then the Projects alphabetically, each with the
 *  count of Models its view shows. A Model in several Projects counts in
 *  each, so the counts are never summed. */
export function LibrarySidebar(props: LibrarySidebarProps) {
  const counts = createMemo(() => viewCounts(props.models, props.now));
  const projects = createMemo(() =>
    [...props.projects].sort((a, b) => a.name.localeCompare(b.name, undefined, { sensitivity: "base" })),
  );
  const isCurrent = (view: LibraryView) => props.view.kind === view.kind && props.view.id === view.id;

  return (
    <nav class={styles.sidebar} aria-label="Library">
      <ul class={styles.list}>
        <For each={SAVED_VIEW_IDS}>
          {(id) => (
            <li>
              <button
                type="button"
                class={styles.entry}
                data-entry
                aria-current={isCurrent({ kind: "view", id }) ? "page" : undefined}
                onClick={() => props.onSelectView({ kind: "view", id })}
              >
                <span class={styles.label}>{SAVED_VIEW_LABELS[id]}</span>
                <span class={styles.count}>{counts()[id]}</span>
              </button>
            </li>
          )}
        </For>
      </ul>
      <h2 class={styles.heading}>Projects</h2>
      <ul class={styles.list}>
        <For each={projects()} fallback={<li class={styles.empty}>No Projects yet</li>}>
          {(project) => (
            <li class={styles.projectRow}>
              <button
                type="button"
                class={styles.entry}
                data-entry
                aria-current={isCurrent({ kind: "project", id: project.id }) ? "page" : undefined}
                onClick={() => props.onSelectView({ kind: "project", id: project.id })}
              >
                <span class={styles.label}>{project.name}</span>
                <span class={styles.count}>
                  {modelsFor({ kind: "project", id: project.id }, props.models, props.now).length}
                </span>
              </button>
              <DropdownMenu
                trigger={
                  <span class={styles.menuTrigger} aria-label={`Actions for ${project.name}`}>
                    <IconDots size={14} aria-hidden="true" />
                  </span>
                }
                items={[
                  { label: "Rename…", onSelect: () => props.onRenameProject(project.id) },
                  { label: "Delete…", onSelect: () => props.onDeleteProject(project.id) },
                ]}
              />
            </li>
          )}
        </For>
      </ul>
    </nav>
  );
}
