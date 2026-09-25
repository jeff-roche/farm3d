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

export function PrinterRoster(props: PrinterRosterProps) {
  const [open, setOpen] = createSignal(false);
  let closeTimeout: ReturnType<typeof setTimeout> | undefined;
  let ignoreRestoredTriggerFocus = false;

  const cancelClose = () => {
    if (closeTimeout !== undefined) {
      clearTimeout(closeTimeout);
      closeTimeout = undefined;
    }
  };

  const openRoster = () => {
    cancelClose();
    setOpen(true);
  };

  const closeRosterSoon = () => {
    cancelClose();
    closeTimeout = setTimeout(() => {
      closeTimeout = undefined;
      setOpen(false);
    }, CLOSE_DELAY_MS);
  };

  const openOnTriggerFocus = () => {
    if (ignoreRestoredTriggerFocus) {
      ignoreRestoredTriggerFocus = false;
      return;
    }
    openRoster();
  };

  const ignoreNextRestoredTriggerFocus = () => {
    ignoreRestoredTriggerFocus = true;
    queueMicrotask(() => {
      ignoreRestoredTriggerFocus = false;
    });
  };

  onCleanup(cancelClose);

  return (
    <KPopover
      open={open()}
      onOpenChange={(nextOpen) => {
        cancelClose();
        setOpen(nextOpen);
      }}
      placement="bottom-start"
    >
      <KPopover.Trigger
        class={styles.trigger}
        aria-label={`${props.count} ${props.label}`}
        onPointerEnter={(event: PointerEvent) => {
          if (event.pointerType !== "touch") openRoster();
        }}
        onPointerLeave={(event: PointerEvent) => {
          if (event.pointerType !== "touch") closeRosterSoon();
        }}
        onFocus={openOnTriggerFocus}
        onBlur={closeRosterSoon}
      >
        <span class={styles.count}>{props.count}</span>
        <span>{props.label}</span>
      </KPopover.Trigger>
      <KPopover.Portal>
        <KPopover.Content
          class={styles.content}
          onPointerEnter={cancelClose}
          onPointerLeave={closeRosterSoon}
          onFocus={cancelClose}
          onBlur={closeRosterSoon}
          onCloseAutoFocus={ignoreNextRestoredTriggerFocus}
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
                setOpen(false);
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
