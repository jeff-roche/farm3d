import { Button as KButton } from "@kobalte/core/button";
import { Popover as KPopover } from "@kobalte/core/popover";
import { For, Show, createSignal, onCleanup } from "solid-js";
import styles from "./PrinterRoster.module.css";

export interface PrinterRosterEntry {
  id: string;
  name: string;
  /** A secondary fact shown after the name, e.g. where the Printer is. */
  detail?: string;
  stateLabel: string;
}

export interface PrinterRosterProps {
  label: string;
  count: number;
  printers: readonly PrinterRosterEntry[];
  onViewAll?: () => void;
}

const ROSTER_LIMIT = 8;
const CLOSE_DELAY_MS = 200;

/** Elements a Tab key can reach, in document order. */
const TABBABLE = [
  "a[href]",
  "button:not([disabled])",
  "input:not([disabled]):not([type='hidden'])",
  "select:not([disabled])",
  "textarea:not([disabled])",
  "[tabindex]",
  "[contenteditable]:not([contenteditable='false'])",
].join(",");

function isTabbable(element: HTMLElement): boolean {
  if (element.tabIndex < 0) return false;
  if (element.closest("[hidden], [inert]")) return false;
  // Not in jsdom; a browser skips what isn't rendered.
  return typeof element.checkVisibility === "function" ? element.checkVisibility() : true;
}

/** The element a forward Tab reaches after `anchor`, skipping `skip`. */
function nextTabbableAfter(anchor: HTMLElement, skip: HTMLElement | undefined): HTMLElement | undefined {
  return Array.from(document.querySelectorAll<HTMLElement>(TABBABLE)).find(
    (element) =>
      !skip?.contains(element) &&
      !anchor.contains(element) &&
      (anchor.compareDocumentPosition(element) & Node.DOCUMENT_POSITION_FOLLOWING) !== 0 &&
      isTabbable(element),
  );
}

/** How the roster was opened: a hover shows it without moving focus; a
 *  press (click, Enter or Space) moves focus into it. Focusing the chip
 *  never opens it, so tabbing past the chip never leaves the page's order. */
type OpenedBy = "hover" | "press";

export function PrinterRoster(props: PrinterRosterProps) {
  const [openedBy, setOpenedBy] = createSignal<OpenedBy | undefined>();
  const open = () => openedBy() !== undefined;
  let trigger: HTMLButtonElement | undefined;
  let content: HTMLDivElement | undefined;
  let closeTimeout: ReturnType<typeof setTimeout> | undefined;
  /** Set by the chip's own click, just before Kobalte toggles. */
  let pressing = false;
  /** Where focus goes when the roster closes, instead of the chip. */
  let focusAfterClose: HTMLElement | null | undefined;
  /** A hovered roster never took focus, so it gives none back. */
  let hoverOnly = false;

  const cancelClose = () => {
    if (closeTimeout !== undefined) {
      clearTimeout(closeTimeout);
      closeTimeout = undefined;
    }
  };

  const close = () => {
    cancelClose();
    setOpenedBy(undefined);
  };

  const closeSoonAfterHover = () => {
    if (openedBy() !== "hover") return;
    cancelClose();
    closeTimeout = setTimeout(() => {
      closeTimeout = undefined;
      if (openedBy() === "hover") setOpenedBy(undefined);
    }, CLOSE_DELAY_MS);
  };

  /** Tab leaves the portalled roster for the chip's neighbours, so the
   *  order runs as if the roster sat right after the chip. */
  const onContentKeyDown = (event: KeyboardEvent) => {
    if (event.key !== "Tab" || !content) return;
    const inside = Array.from(content.querySelectorAll<HTMLElement>(TABBABLE)).filter(isTabbable);
    const active = document.activeElement;
    const atStart = active === content || active === inside[0];
    const atEnd = active === content || inside.length === 0 || active === inside[inside.length - 1];
    if (event.shiftKey && atStart) {
      event.preventDefault();
      focusAfterClose = trigger;
      close();
    } else if (!event.shiftKey && atEnd && trigger) {
      event.preventDefault();
      focusAfterClose = nextTabbableAfter(trigger, content) ?? null;
      close();
    }
  };

  onCleanup(cancelClose);

  return (
    <KPopover
      open={open()}
      onOpenChange={(nextOpen) => {
        cancelClose();
        const pressed = pressing;
        pressing = false;
        // A click on a hovered chip keeps the roster open.
        if (!nextOpen && pressed && openedBy() === "hover") {
          hoverOnly = false;
          setOpenedBy("press");
        } else setOpenedBy(nextOpen ? "press" : undefined);
      }}
      placement="bottom-start"
    >
      <KPopover.Trigger
        ref={trigger}
        class={styles.trigger}
        aria-label={`${props.count} ${props.label}`}
        onClick={() => {
          pressing = true;
        }}
        onPointerEnter={(event: PointerEvent) => {
          if (event.pointerType === "touch") return;
          cancelClose();
          if (!open()) setOpenedBy("hover");
        }}
        onPointerLeave={(event: PointerEvent) => {
          if (event.pointerType !== "touch") closeSoonAfterHover();
        }}
      >
        <span class={styles.count}>{props.count}</span>
        <span>{props.label}</span>
      </KPopover.Trigger>
      <KPopover.Portal>
        <KPopover.Content
          ref={content}
          class={styles.content}
          onPointerEnter={cancelClose}
          onPointerLeave={closeSoonAfterHover}
          onKeyDown={onContentKeyDown}
          onOpenAutoFocus={(event: Event) => {
            hoverOnly = openedBy() === "hover";
            if (hoverOnly) event.preventDefault();
          }}
          onCloseAutoFocus={(event: Event) => {
            const target = focusAfterClose;
            focusAfterClose = undefined;
            if (hoverOnly && target === undefined) {
              event.preventDefault();
            } else if (target !== undefined) {
              event.preventDefault();
              target?.focus();
            }
          }}
        >
          <div class={styles.heading}>{props.label}</div>
          <Show
            when={props.printers.length > 0}
            fallback={<p class={styles.empty}>No Printers in this roster.</p>}
          >
            <ul class={styles.list}>
              <For each={props.printers.slice(0, ROSTER_LIMIT)}>
                {(printer) => (
                  <li class={styles.row}>
                    <span class={styles.name}>
                      {printer.name}
                      <Show when={printer.detail}>
                        {(detail) => <span class={styles.detail}> · {detail()}</span>}
                      </Show>
                    </span>
                    <span class={styles.state}>{printer.stateLabel}</span>
                  </li>
                )}
              </For>
            </ul>
          </Show>
          <Show when={props.count > ROSTER_LIMIT}>
            <KButton
              class={styles.viewAll}
              onClick={() => {
                close();
                props.onViewAll?.();
              }}
            >
              View all
            </KButton>
          </Show>
        </KPopover.Content>
      </KPopover.Portal>
    </KPopover>
  );
}
