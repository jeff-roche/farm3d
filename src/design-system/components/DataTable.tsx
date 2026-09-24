import { For, Show, createMemo, type JSX } from "solid-js";
import styles from "./DataTable.module.css";

export interface DataTableColumn<T> {
  id: string;
  header: string;
  cell: (row: T) => JSX.Element;
  /** When present, the column's header becomes clickable and can drive
   *  `sort`/`onSortChange`. Sorting the `rows` array itself is the caller's
   *  job (from `sort`, via `sortValue`) — `DataTable` never reorders rows
   *  on its own. */
  sortValue?: (row: T) => string | number;
  width?: string;
  align?: "start" | "end";
}

export interface DataTableSort {
  columnId: string;
  direction: "asc" | "desc";
}

export interface DataTableProps<T> {
  rows: T[];
  rowId: (row: T) => string;
  columns: DataTableColumn<T>[];
  selectedId?: string | null;
  onSelect?: (id: string) => void;
  onActivate?: (id: string) => void;
  sort?: DataTableSort;
  onSortChange?: (sort: DataTableSort) => void;
  label: string;
  empty?: JSX.Element;
}

/** A dense, sortable, keyboard-navigable data grid (ARIA `grid` pattern) for
 *  tabular record lists — Spool inventory now, the P7 Queue and P4 Library
 *  later. It never reorders or filters `rows` itself: `sort` is reported
 *  through `onSortChange` and applied by the caller, and selection is fully
 *  controlled through `selectedId`/`onSelect`.
 *
 *  The table wrapper scrolls horizontally on overflow (never the page), and
 *  its header is sticky. */
export function DataTable<T>(props: DataTableProps<T>): JSX.Element {
  const rowIds = createMemo(() => props.rows.map((row) => props.rowId(row)));
  const rowRefs = new Map<string, HTMLTableRowElement>();

  const currentIndex = createMemo(() => {
    if (props.selectedId == null) return -1;
    return rowIds().indexOf(props.selectedId);
  });

  const moveTo = (index: number) => {
    const ids = rowIds();
    if (ids.length === 0) return;
    const clamped = Math.max(0, Math.min(index, ids.length - 1));
    const id = ids[clamped];
    if (id !== undefined) {
      props.onSelect?.(id);
      // Roving tabindex: move real DOM focus onto the newly-active row so a
      // keyboard user's Tab stop tracks selection, not just `aria-selected`.
      rowRefs.get(id)?.focus();
    }
  };

  const handleKeyDown: JSX.EventHandler<HTMLTableElement, KeyboardEvent> = (event) => {
    // Row navigation only — a header's sort button lives in `<thead>` and
    // handles its own Enter/Space via native button activation; without
    // this guard, arrow keys and Enter on a focused header would also move
    // row selection / re-fire onActivate.
    const target = event.target as HTMLElement | null;
    if (target?.closest("thead")) return;

    const ids = rowIds();
    if (ids.length === 0) return;

    switch (event.key) {
      case "ArrowDown":
        event.preventDefault();
        moveTo(currentIndex() < 0 ? 0 : currentIndex() + 1);
        break;
      case "ArrowUp":
        event.preventDefault();
        moveTo(currentIndex() < 0 ? 0 : currentIndex() - 1);
        break;
      case "Home":
        event.preventDefault();
        moveTo(0);
        break;
      case "End":
        event.preventDefault();
        moveTo(ids.length - 1);
        break;
      case "Enter": {
        const id = props.selectedId;
        if (id != null) {
          event.preventDefault();
          props.onActivate?.(id);
        }
        break;
      }
      default:
        break;
    }
  };

  const ariaSortFor = (columnId: string): "ascending" | "descending" | "none" => {
    if (!props.sort || props.sort.columnId !== columnId) return "none";
    return props.sort.direction === "asc" ? "ascending" : "descending";
  };

  const toggleSort = (column: DataTableColumn<T>) => {
    if (!column.sortValue || !props.onSortChange) return;
    const direction: "asc" | "desc" =
      props.sort?.columnId === column.id && props.sort.direction === "asc" ? "desc" : "asc";
    props.onSortChange({ columnId: column.id, direction });
  };

  return (
    <div class={styles.wrapper}>
      <table
        class={styles.table}
        role="grid"
        aria-label={props.label}
        aria-rowcount={1 + (props.rows.length > 0 ? props.rows.length : props.empty ? 1 : 0)}
        tabIndex={currentIndex() < 0 && props.rows.length > 0 ? 0 : -1}
        onKeyDown={handleKeyDown}
      >
        <thead class={styles.head}>
          {/* The header row is row 1, so data rows start at 2. */}
          <tr role="row" aria-rowindex={1}>
            <For each={props.columns}>
              {(column) => (
                <th
                  role="columnheader"
                  scope="col"
                  class={styles.headerCell}
                  style={column.width ? { width: column.width } : undefined}
                  data-align={column.align}
                  aria-sort={column.sortValue ? ariaSortFor(column.id) : undefined}
                >
                  <Show when={column.sortValue} fallback={column.header}>
                    <button
                      type="button"
                      class={styles.sortButton}
                      onClick={() => toggleSort(column)}
                    >
                      {column.header}
                    </button>
                  </Show>
                </th>
              )}
            </For>
          </tr>
        </thead>
        <tbody>
          <Show
            when={props.rows.length > 0}
            fallback={
              <Show when={props.empty}>
                <tr role="row" aria-rowindex={2}>
                  <td role="gridcell" class={styles.emptyCell} colSpan={props.columns.length}>
                    {props.empty}
                  </td>
                </tr>
              </Show>
            }
          >
            <For each={props.rows}>
              {(row, index) => {
                const id = () => props.rowId(row);
                const selected = () => props.selectedId != null && props.selectedId === id();

                return (
                  <tr
                    ref={(el) => rowRefs.set(id(), el)}
                    role="row"
                    class={styles.row}
                    aria-rowindex={index() + 2}
                    aria-selected={props.selectedId == null ? undefined : selected()}
                    data-selected={selected() ? "" : undefined}
                    tabIndex={selected() ? 0 : -1}
                    onClick={() => props.onSelect?.(id())}
                  >
                    <For each={props.columns}>
                      {(column) => (
                        <td
                          role="gridcell"
                          class={styles.cell}
                          data-align={column.align}
                        >
                          {column.cell(row)}
                        </td>
                      )}
                    </For>
                  </tr>
                );
              }}
            </For>
          </Show>
        </tbody>
      </table>
    </div>
  );
}
