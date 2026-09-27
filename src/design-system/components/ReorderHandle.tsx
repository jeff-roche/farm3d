import { IconDotsVertical, IconGripVertical } from "@tabler/icons-solidjs";
import { createSignal, onCleanup, Show } from "solid-js";
import { DropdownMenu, type DropdownMenuEntry } from "./DropdownMenu";
import styles from "./ReorderHandle.module.css";

export interface ReorderHandleProps {
  /** The moved item's name, used in the accessible name and the live-region announcement. */
  label: string;
  /** This item's current position (0-based). */
  index: number;
  /** The number of items in the reorderable list. */
  count: number;
  /** Called once per committed move, never for a no-op (moving an item to
   *  the position it already holds). */
  onMove: (from: number, to: number) => void;
  disabled?: boolean;
}

function clamp(value: number, count: number): number {
  return Math.min(Math.max(value, 0), count - 1);
}

/** A drag handle for reordering a list, with a keyboard path and a menu so
 *  reordering never depends on pointer drag alone:
 *
 *  - **Keyboard** (on the handle itself): Alt+ArrowUp/ArrowDown move by one;
 *    Alt+Home/End move to the top/bottom. Clamped moves that don't change
 *    position emit nothing.
 *  - **Pointer**: a drag from the handle shows a drop indicator and commits
 *    the move on pointerup; Escape cancels. A second pointerdown while a
 *    drag is already active is ignored, so window listeners from the first
 *    drag are never orphaned by a second set.
 *
 *    **Row geometry contract**: each row in the reorderable list should
 *    carry a `data-reorder-row` attribute on the element that's a sibling
 *    of the other rows (their common parent's direct children), in the
 *    same order as `index`/`count`. At pointerdown, the handle walks up to
 *    its own `[data-reorder-row]` via `closest()`, snapshots every sibling
 *    row's `getBoundingClientRect()` once, and during the drag picks the
 *    target row by which rows' vertical midpoints the pointer has passed —
 *    this handles rows of different heights correctly, unlike a uniform
 *    step size. If no `[data-reorder-row]` ancestor exists, it falls back
 *    to treating its own measured height as a uniform row unit (steps of
 *    `round(deltaY / ownHeight)`) — that fallback is what makes the
 *    Showcase's `ReorderHandleDemo` and this component's simpler tests
 *    work without any row markup at all.
 *  - **Menu**: a companion `DropdownMenu` with "Move to top/up/down/to
 *    bottom", disabled at either end. When `disabled`, the trigger stays
 *    present (not hidden) but is inert — Kobalte's own `disabled` handling
 *    blocks it from opening, and it's dimmed like the handle.
 *
 *  A polite live region announces "Moved <label> to position N of M" after
 *  every committed move, however it was made. */
export function ReorderHandle(props: ReorderHandleProps) {
  let handleRef: HTMLButtonElement | undefined;
  const [dragging, setDragging] = createSignal(false);
  const [targetIndex, setTargetIndex] = createSignal<number | null>(null);
  const [dropOffset, setDropOffset] = createSignal(0);
  const [announcement, setAnnouncement] = createSignal("");

  let activeCleanup: (() => void) | undefined;
  onCleanup(() => activeCleanup?.());

  function commit(target: number) {
    const from = props.index;
    if (target === from) return;
    props.onMove(from, target);
    setAnnouncement(`Moved ${props.label} to position ${target + 1} of ${props.count}`);
  }

  function onKeyDown(event: KeyboardEvent) {
    if (props.disabled || !event.altKey) return;
    let target: number;
    switch (event.key) {
      case "ArrowUp":
        target = clamp(props.index - 1, props.count);
        break;
      case "ArrowDown":
        target = clamp(props.index + 1, props.count);
        break;
      case "Home":
        target = 0;
        break;
      case "End":
        target = props.count - 1;
        break;
      default:
        return;
    }
    event.preventDefault();
    commit(target);
  }

  /** Direct-child `[data-reorder-row]` siblings of `row`'s parent, in DOM order. */
  function siblingRows(row: HTMLElement): HTMLElement[] {
    const parent = row.parentElement;
    if (!parent) return [];
    return Array.from(parent.children).filter(
      (child): child is HTMLElement => child instanceof HTMLElement && child.hasAttribute("data-reorder-row"),
    );
  }

  /** How many of `rects`' vertical midpoints the pointer has passed, clamped to the list. */
  function targetFromMidpoints(rects: DOMRect[], pointerY: number): number {
    let target = 0;
    for (const rect of rects) {
      if (pointerY > rect.top + rect.height / 2) target++;
    }
    return clamp(target, props.count);
  }

  function onPointerDown(event: PointerEvent) {
    if (props.disabled || dragging() || event.button !== 0 || !handleRef) return;
    event.preventDefault();

    const startIndex = props.index;
    const row = handleRef.closest<HTMLElement>("[data-reorder-row]");
    const rows = row ? siblingRows(row) : [];

    setDragging(true);
    setTargetIndex(startIndex);
    setDropOffset(0);
    handleRef.setPointerCapture(event.pointerId);

    let onPointerMove: (moveEvent: PointerEvent) => void;

    if (rows.length > 0) {
      // Real row geometry: snapshot every sibling row's rect once, then pick
      // the target by which rows' midpoints the pointer has passed. Correct
      // for rows of uneven height, unlike a uniform step size.
      const rects = rows.map((r) => r.getBoundingClientRect());
      const ownTop = (rects[startIndex] ?? rects[0]).top;
      onPointerMove = (moveEvent) => {
        const next = targetFromMidpoints(rects, moveEvent.clientY);
        setTargetIndex(next);
        setDropOffset((rects[next]?.top ?? ownTop) - ownTop);
      };
    } else {
      // Fallback: no `[data-reorder-row]` ancestor, so there's no sibling
      // geometry to read. Treat the handle's own measured height as a
      // uniform row unit.
      const startY = event.clientY;
      const rowHeight = handleRef.getBoundingClientRect().height || 1;
      onPointerMove = (moveEvent) => {
        const steps = Math.round((moveEvent.clientY - startY) / rowHeight);
        const next = clamp(startIndex + steps, props.count);
        setTargetIndex(next);
        setDropOffset((next - startIndex) * rowHeight);
      };
    }

    const finish = (commitMove: boolean) => {
      window.removeEventListener("pointermove", onPointerMove);
      window.removeEventListener("pointerup", onPointerUp);
      window.removeEventListener("keydown", onEscape);
      handleRef?.releasePointerCapture(event.pointerId);
      activeCleanup = undefined;
      setDragging(false);
      const final = targetIndex();
      setTargetIndex(null);
      if (commitMove && final !== null) commit(final);
    };

    const onPointerUp = () => finish(true);
    const onEscape = (keyEvent: KeyboardEvent) => {
      if (keyEvent.key !== "Escape") return;
      keyEvent.preventDefault();
      finish(false);
    };

    window.addEventListener("pointermove", onPointerMove);
    window.addEventListener("pointerup", onPointerUp);
    window.addEventListener("keydown", onEscape);
    activeCleanup = () => finish(false);
  }

  const menuItems = (): DropdownMenuEntry[] => [
    { label: "Move to top", onSelect: () => commit(0), disabled: props.index === 0 },
    { label: "Move up", onSelect: () => commit(clamp(props.index - 1, props.count)), disabled: props.index === 0 },
    {
      label: "Move down",
      onSelect: () => commit(clamp(props.index + 1, props.count)),
      disabled: props.index === props.count - 1,
    },
    { label: "Move to bottom", onSelect: () => commit(props.count - 1), disabled: props.index === props.count - 1 },
  ];

  return (
    <div class={styles.root}>
      <button
        ref={handleRef}
        type="button"
        class={styles.handle}
        aria-label={`Reorder ${props.label}`}
        disabled={props.disabled}
        onPointerDown={onPointerDown}
        onKeyDown={onKeyDown}
      >
        <IconGripVertical aria-hidden="true" size={16} />
      </button>
      <Show when={dragging()}>
        <div class={styles.dropIndicator} style={{ transform: `translateY(${dropOffset()}px)` }} />
      </Show>
      <DropdownMenu
        trigger={
          <span class={styles.menuTrigger} aria-label={`Move ${props.label}`}>
            <IconDotsVertical aria-hidden="true" size={16} />
          </span>
        }
        items={menuItems()}
        disabled={props.disabled}
      />
      <p class={styles.live} aria-live="polite">{announcement()}</p>
    </div>
  );
}
