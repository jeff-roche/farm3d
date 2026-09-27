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
 *    the move on pointerup; Escape cancels. The handle is expected to fill
 *    its row's height (it stretches via `align-self: stretch` in a flex
 *    row) so its own measured height stands in for row height — there's no
 *    other row geometry this component can reach, since it only knows about
 *    the row it belongs to.
 *  - **Menu**: a companion `DropdownMenu` with "Move to top/up/down/to
 *    bottom", disabled at either end.
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

  function onPointerDown(event: PointerEvent) {
    if (props.disabled || event.button !== 0 || !handleRef) return;
    event.preventDefault();

    const startIndex = props.index;
    const startY = event.clientY;
    const rowHeight = handleRef.getBoundingClientRect().height || 1;

    setDragging(true);
    setTargetIndex(startIndex);
    setDropOffset(0);
    handleRef.setPointerCapture(event.pointerId);

    const onPointerMove = (moveEvent: PointerEvent) => {
      const steps = Math.round((moveEvent.clientY - startY) / rowHeight);
      const next = clamp(startIndex + steps, props.count);
      setTargetIndex(next);
      setDropOffset((next - startIndex) * rowHeight);
    };

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
      <Show
        when={!props.disabled}
        fallback={
          <span class={styles.menuTrigger} aria-hidden="true">
            <IconDotsVertical aria-hidden="true" size={16} />
          </span>
        }
      >
        <DropdownMenu
          trigger={
            <span class={styles.menuTrigger} aria-label={`Move ${props.label}`}>
              <IconDotsVertical aria-hidden="true" size={16} />
            </span>
          }
          items={menuItems()}
        />
      </Show>
      <p class={styles.live} aria-live="polite">{announcement()}</p>
    </div>
  );
}
