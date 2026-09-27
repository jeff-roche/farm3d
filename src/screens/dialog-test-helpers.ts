/** Test-only keyboard helpers for the Job dialogs (nothing outside a test
 *  imports this module). jsdom doesn't move focus on Tab, so "Tab order"
 *  is checked as the dialog's tabbable elements in document order -- the
 *  order a real Tab walks them -- and Escape through Kobalte's own
 *  dismiss handler. */
import { fireEvent } from "@solidjs/testing-library";

const TABBABLE = "button, input, select, textarea, [tabindex]:not([tabindex='-1']), a[href]";

/** The accessible names of `dialog`'s enabled, tabbable controls, in Tab
 *  order. A Kobalte checkbox or radio is named by its `<label>`. */
export function tabOrder(dialog: HTMLElement): string[] {
  return [...dialog.querySelectorAll<HTMLElement>(TABBABLE)]
    // Kobalte's focus-trap sentinels are tabbable but never a stop.
    .filter((element) => !element.hasAttribute("data-focus-trap"))
    .filter((element) => !(element as HTMLButtonElement).disabled && element.tabIndex >= 0)
    .filter((element) => !(element instanceof HTMLInputElement && element.type === "radio" && !radioTabStop(dialog, element)))
    .map((element) => nameOf(element));
}

/** A radio group is one Tab stop: its checked radio, or its first when
 *  none is checked (what a browser does). */
function radioTabStop(dialog: HTMLElement, radio: HTMLInputElement): boolean {
  const group = [...dialog.querySelectorAll<HTMLInputElement>(`input[type=radio][name="${radio.name}"]`)].filter((r) => !r.disabled);
  const checked = group.find((r) => r.checked);
  return checked ? checked === radio : group[0] === radio;
}

function nameOf(element: HTMLElement): string {
  const label = element.getAttribute("aria-label");
  if (label) return label;
  const labelledBy = element.getAttribute("aria-labelledby");
  if (labelledBy) {
    const text = labelledBy.split(" ").map((id) => element.ownerDocument.getElementById(id)?.textContent ?? "").join(" ").trim();
    if (text) return text;
  }
  if (element instanceof HTMLInputElement && element.labels && element.labels.length > 0) {
    return element.labels[0].textContent?.trim() ?? "";
  }
  return element.textContent?.trim() ?? "";
}

/** Presses Escape inside the dialog, where Kobalte listens for it. */
export function pressEscape(dialog: HTMLElement): void {
  fireEvent.keyDown(dialog, { key: "Escape" });
}
