import { RadioGroup as KRadioGroup } from "@kobalte/core/radio-group";
import { createEffect, createSignal, For, on, onCleanup } from "solid-js";
import { Button, useTheme, type ThemeMode } from "../../design-system";
import styles from "./Settings.module.css";

const MODES: { value: ThemeMode; label: string }[] = [
  { value: "system", label: "System" },
  { value: "farm3d-light", label: "Light" },
  { value: "farm3d-dark", label: "Dark" },
];

/** Appearance category: selecting an option previews it live, Apply commits
 *  it, Revert (or leaving the category, or the workspace) restores the
 *  committed theme. The same preview semantics the theme popover had. */
export function AppearanceSettings() {
  const theme = useTheme();
  const [pendingMode, setPendingMode] = createSignal<ThemeMode>(theme.mode());
  const dirty = () => pendingMode() !== theme.mode();

  // A theme committed elsewhere (a settings import) becomes the selection.
  createEffect(on(theme.mode, (committed) => setPendingMode(committed), { defer: true }));

  // Unmounting (another category, another screen) drops any pending preview.
  onCleanup(() => theme.cancelPreview());

  function handleCandidate(value: ThemeMode) {
    setPendingMode(value);
    theme.previewTheme(value);
  }

  function handleApply() {
    theme.setThemeMode(pendingMode());
  }

  function handleRevert() {
    setPendingMode(theme.mode());
    theme.cancelPreview();
  }

  return (
    <div class={styles.category}>
      <h3 class={styles.heading}>Appearance</h3>
      <p class={styles.hint}>Click an option to preview it.</p>
      <KRadioGroup value={pendingMode()} onChange={handleCandidate} class={styles.radioList} aria-label="Theme">
        <For each={MODES}>
          {(mode) => (
            <KRadioGroup.Item value={mode.value} class={styles.radioItem}>
              <KRadioGroup.ItemInput />
              <KRadioGroup.ItemControl class={styles.radioControl} />
              <KRadioGroup.ItemLabel class={styles.radioLabel}>{mode.label}</KRadioGroup.ItemLabel>
            </KRadioGroup.Item>
          )}
        </For>
      </KRadioGroup>
      <div class={styles.actions}>
        <Button variant="ghost" disabled={!dirty()} onClick={handleRevert}>
          Revert
        </Button>
        <Button variant="primary" disabled={!dirty()} onClick={handleApply}>
          Apply
        </Button>
      </div>
    </div>
  );
}
