import { cleanup, fireEvent, render, screen, waitFor, within } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import { webRestorePreview } from "../../backup/web-fixtures";
import type { RestorePreview } from "../../backup/types";
import { RestorePreviewPanel } from "./RestorePreviewPanel";

afterEach(cleanup);

function panel(overrides: Partial<RestorePreview> = {}, props: Partial<Parameters<typeof RestorePreviewPanel>[0]> = {}) {
  const handlers = { onRestore: vi.fn(), onCancel: vi.fn(), onConfirm: vi.fn(), onCancelConfirm: vi.fn(), onOpenQueue: vi.fn() };
  render(() => <RestorePreviewPanel preview={{ ...webRestorePreview(), ...overrides }} confirming={false} {...handlers} {...props} />);
  return handlers;
}

describe("RestorePreviewPanel", () => {
  it("shows the backup's facts and the counts side by side", () => {
    panel();
    expect(screen.getByText(/farm-2026-09-20\.farm3d-backup/)).toBeTruthy();
    const table = screen.getByRole("grid", { name: "Row counts" });
    expect(within(table).getByText("This Farm")).toBeTruthy();
    expect(within(table).getByText("Backup")).toBeTruthy();
    const jobs = within(table).getByText("jobs").closest("tr")!;
    expect(within(jobs).getByText("6")).toBeTruthy();
    expect(within(jobs).getByText("5")).toBeTruthy();
  });

  it("groups conflicts by class and domain with totals beyond the cap", () => {
    panel({
      conflicts: [
        { class: "onlyLocal", domain: "job", total: 250, items: Array.from({ length: 200 }, (_, i) => ({ localId: `j${i}`, backupId: null, label: `Job ${i}` })) },
        { class: "changed", domain: "printer", total: 1, items: [{ localId: "p1", backupId: "p1", label: "Bay 4" }] },
      ],
    });
    expect(screen.getByRole("heading", { name: /Only on this Farm/ })).toBeTruthy();
    expect(screen.getByRole("heading", { name: /Changed/ })).toBeTruthy();
    expect(screen.getByText("and 50 more")).toBeTruthy();
    expect(screen.getByText("Bay 4")).toBeTruthy();
    expect(screen.getByText(/Jobs/).textContent).toContain("250");
  });

  it("lists the notices as text", () => {
    panel();
    expect(screen.getByText(/needs its credentials|need its credentials/)).toBeTruthy();
    expect(screen.getByText(/slicer setup on this computer is kept/i)).toBeTruthy();
  });

  it("disables Restore for blockers and links to the Queue", () => {
    const handlers = panel({ blockers: [{ kind: "activeJob", id: "job-1" }], blockerTotal: 53 });
    expect((screen.getByRole("button", { name: "Restore" }) as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getByText("An active Job")).toBeTruthy();
    expect(screen.getByText(/and 52 more/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Open the Queue" }));
    expect(handlers.onOpenQueue).toHaveBeenCalled();
  });

  it("Restore asks the store to confirm, and the dialog needs the phrase restore", async () => {
    const handlers = panel({}, { confirming: true });
    expect(await screen.findByRole("dialog")).toBeTruthy();
    expect(screen.getByRole("dialog").textContent).toMatch(/farm3d will restart/);
    expect(screen.getByRole("dialog").textContent).toMatch(/safety backup/i);
    const confirm = screen.getByRole("button", { name: "Restore and restart" }) as HTMLButtonElement;
    expect(confirm.disabled).toBe(true);
    fireEvent.input(screen.getByLabelText('Type "restore" to confirm'), { target: { value: "restore" } });
    await waitFor(() => expect(confirm.disabled).toBe(false));
    fireEvent.click(confirm);
    expect(handlers.onConfirm).toHaveBeenCalledWith("restore");
  });

  it("presses Restore to begin confirming", () => {
    const handlers = panel();
    fireEvent.click(screen.getByRole("button", { name: "Restore" }));
    expect(handlers.onRestore).toHaveBeenCalled();
  });
});
