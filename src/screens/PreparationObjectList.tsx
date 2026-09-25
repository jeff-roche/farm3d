import { Show } from "solid-js";
import { DataTable, SeverityMarker, type DataTableColumn } from "../design-system";
import type { PlacementCheck } from "../slicing/bounds";
import type { InstanceDoc } from "../slicing/types";
import styles from "./PreparationObjectList.module.css";

/** D18: above this many triangles on a plate, the viewport may drop
 *  frames. */
export const LARGE_PLATE_TRIANGLES = 2_000_000;

export interface PreparationObjectListProps {
  instances: InstanceDoc[];
  instanceName: (instanceKey: string) => string;
  placement: (instanceKey: string) => PlacementCheck | undefined;
  /** The placement shown predates the instance's latest turn or scale. */
  checking?: (instanceKey: string) => boolean;
  /** The last arrange couldn't fit this instance and left it in place. */
  notArranged?: (instanceKey: string) => boolean;
  selectedInstanceKey: string | null;
  onSelect: (instanceKey: string) => void;
  /** The plate's triangle count, for the large-model note. */
  triangleCount: number;
  /** Keyboard commands (D19), while the list has focus. */
  onKeyDown?: (event: KeyboardEvent) => void;
}

/** What is wrong with an instance's placement, in words (D18: colour is
 *  never the only signal). */
export function placementProblem(check: PlacementCheck | undefined): string | undefined {
  if (!check) return undefined;
  const problems = [
    check.outOfBounds ? "outside the bed" : undefined,
    check.inExcludeArea ? "in an exclude area" : undefined,
    check.tooTall ? "too tall" : undefined,
  ].filter(Boolean);
  if (problems.length === 0) return undefined;
  const text = problems.join(", ");
  return text[0].toUpperCase() + text.slice(1);
}

const mm = (value: number) => value.toFixed(1);

/** D19's object list: the shown plate's objects, one row each, selectable
 *  with the arrow keys; placement problems are named in text. */
export function PreparationObjectList(props: PreparationObjectListProps) {
  // Rows are instance keys, which stay put while an instance is edited, so
  // a focused row isn't rebuilt under the keyboard.
  const instance = (key: string) => props.instances.find((candidate) => candidate.instanceKey === key);
  const columns: DataTableColumn<string>[] = [
    { id: "name", header: "Object", cell: (key) => props.instanceName(key) },
    {
      id: "position",
      header: "Position (mm)",
      cell: (key) => {
        const [x, y] = instance(key)?.transform.translateMm ?? [0, 0];
        return <span class={styles.numbers}>X {mm(x)}, Y {mm(y)}</span>;
      },
    },
    {
      id: "status",
      header: "Placement",
      cell: (key) => (
        <span class={styles.placement}>
          <Show
            when={!props.checking?.(key)}
            fallback={<span class={styles.ok}>Checking placement…</span>}
          >
            <Show when={placementProblem(props.placement(key))} fallback={<span class={styles.ok}>On the bed</span>}>
              {(problem) => <SeverityMarker severity="warning" label={problem()} />}
            </Show>
          </Show>
          <Show when={props.notArranged?.(key)}>
            <SeverityMarker severity="warning" label="Didn't fit, not arranged" />
          </Show>
        </span>
      ),
    },
  ];

  return (
    <div class={styles.list} onKeyDown={(event) => props.onKeyDown?.(event)}>
      <DataTable
        label="Objects on this plate"
        rows={props.instances.map((candidate) => candidate.instanceKey)}
        rowId={(key) => key}
        columns={columns}
        selectedId={props.selectedInstanceKey}
        onSelect={props.onSelect}
        empty={<span class={styles.ok}>No objects on this plate. Move one here, or delete the plate.</span>}
      />
      <Show when={props.triangleCount > LARGE_PLATE_TRIANGLES}>
        <p class={styles.note}>Large model: viewport may be slow</p>
      </Show>
    </div>
  );
}
