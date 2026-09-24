import { Show } from "solid-js";
import { DataTable, SeverityMarker, type DataTableColumn } from "../design-system";
import type { LibraryView } from "../library/saved-views";
import type { ModelRecord, ProjectRecord } from "../library/types";
import {
  formatDate,
  formatLabel,
  projectsSummary,
  sourceStateMarker,
  storageLabel,
} from "./library-presentation";
import { ModelActionsMenu } from "./ModelActionsMenu";
import styles from "./ModelList.module.css";

export interface ModelListProps {
  /** Already filtered and sorted by the caller. */
  models: ModelRecord[];
  projects: ProjectRecord[];
  view: LibraryView;
  selectedId: string | null;
  onSelect: (modelId: string) => void;
  onAddToProject: (modelId: string) => void;
}

/** D19 list mode: P3's `DataTable`, which brings arrow-key and Enter row
 *  selection and scrolls horizontally inside its own wrapper. The toolbar's
 *  sort orders `models`, so no column is sortable here. */
export function ModelList(props: ModelListProps) {
  const columns: DataTableColumn<ModelRecord>[] = [
    { id: "name", header: "Name", cell: (m) => <span class={styles.name}>{m.name}</span> },
    { id: "projects", header: "Projects", cell: (m) => projectsSummary(m, props.projects) },
    { id: "format", header: "Format", cell: (m) => formatLabel(m.format) },
    { id: "storage", header: "Storage", cell: (m) => storageLabel(m) },
    {
      id: "source",
      header: "Source",
      cell: (m) => (
        <Show when={m.link} fallback={<span class={styles.muted}>—</span>}>
          {(link) => <SeverityMarker {...sourceStateMarker(link().state)} />}
        </Show>
      ),
    },
    { id: "revisions", header: "Revisions", align: "end", cell: (m) => String(m.revisionCount) },
    { id: "added", header: "Added", cell: (m) => formatDate(m.createdAt) },
    {
      id: "actions",
      header: "Actions",
      width: "4rem",
      cell: (m) => (
        <ModelActionsMenu model={m} view={props.view} projects={props.projects} onAddToProject={props.onAddToProject} />
      ),
    },
  ];

  return (
    <div class={styles.pane}>
      <DataTable
        label="Models"
        rows={props.models}
        rowId={(m) => m.id}
        columns={columns}
        selectedId={props.selectedId}
        onSelect={props.onSelect}
        onActivate={props.onSelect}
      />
    </div>
  );
}
