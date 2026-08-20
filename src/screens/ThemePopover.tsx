import { RadioGroup as KRadioGroup } from "@kobalte/core/radio-group";
import { createEffect, createSignal, For, on, type Accessor } from "solid-js";
import { Button, Popover, useTheme, type ThemeMode } from "../design-system";
import styles from "./ThemePopover.module.css";

const MODES: { value: ThemeMode; label: string }[] = [
  { value: "system", label: "System" },
  { value: "farm3d-light", label: "Light" },
  { value: "farm3d-dark", label: "Dark" },
];

export interface ThemePopoverProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Anchors the popover to an element other than its own trigger — e.g. the gear icon that opened it. */
  anchorRef?: Accessor<HTMLElement | undefined>;
}

/** Theme picker: clicking or keyboard-arrowing to an option previews it live; Apply commits it. Closing any other way reverts the preview. */
export function ThemePopover(props: ThemePopoverProps) {
  const theme = useTheme();
  const [pendingMode, setPendingMode] = createSignal<ThemeMode>(theme.mode());

  createEffect(
    on(
      () => props.open,
      (isOpen) => {
        if (isOpen) {
          setPendingMode(theme.mode());
        } else {
          theme.cancelPreview();
        }
      },
      { defer: true },
    ),
  );

  function handleCandidate(value: ThemeMode) {
    setPendingMode(value);
    theme.previewTheme(value);
  }

  function handleApply() {
    theme.setThemeMode(pendingMode());
    props.onOpenChange(false);
  }

  return (
    <Popover
      open={props.open}
      onOpenChange={props.onOpenChange}
      anchorRef={props.anchorRef}
      modal
    >
      <span class={styles.title}>Theme</span>
      <span class={styles.hint}>Click an option to preview it.</span>
      <KRadioGroup value={pendingMode()} onChange={handleCandidate} class={styles.list}>
        <For each={MODES}>
          {(mode) => (
            <KRadioGroup.Item value={mode.value} class={styles.item}>
              <KRadioGroup.ItemInput />
              <KRadioGroup.ItemControl class={styles.control} />
              <KRadioGroup.ItemLabel class={styles.itemLabel}>{mode.label}</KRadioGroup.ItemLabel>
            </KRadioGroup.Item>
          )}
        </For>
      </KRadioGroup>
      <div class={styles.actions}>
        <Button variant="primary" onClick={handleApply}>
          Apply
        </Button>
      </div>
    </Popover>
  );
}
