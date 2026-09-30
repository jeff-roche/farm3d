import { createSignal } from "solid-js";
import { fireEvent, render, screen } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import { Tabs } from "./Tabs";
import { TypedConfirmDialog } from "./TypedConfirmDialog";

afterEach(() => {
  document.body.innerHTML = "";
});

const items = [
  { value: "a", label: "Alpha", content: "Alpha body" },
  { value: "b", label: "Beta", content: "Beta body" },
  { value: "c", label: "Gamma", content: "Gamma body" },
];

describe("vertical Tabs", () => {
  it("exposes aria-orientation and moves with ArrowDown / ArrowUp", () => {
    render(() => <Tabs items={items} orientation="vertical" defaultValue="a" />);
    expect(screen.getByRole("tablist")).toHaveAttribute("aria-orientation", "vertical");
    const alpha = screen.getByRole("tab", { name: "Alpha" });
    alpha.focus();
    fireEvent.keyDown(alpha, { key: "ArrowDown" });
    expect(screen.getByRole("tab", { name: "Beta" })).toHaveAttribute("aria-selected", "true");
    fireEvent.keyDown(screen.getByRole("tab", { name: "Beta" }), { key: "ArrowUp" });
    expect(screen.getByRole("tab", { name: "Alpha" })).toHaveAttribute("aria-selected", "true");
  });

  it("defaults to horizontal", () => {
    render(() => <Tabs items={items} defaultValue="a" />);
    expect(screen.getByRole("tablist")).toHaveAttribute("aria-orientation", "horizontal");
  });
});

describe("TypedConfirmDialog", () => {
  function setup(extra: { pending?: boolean } = {}) {
    const onConfirm = vi.fn();
    const [open, setOpen] = createSignal(true);
    render(() => (
      <>
        <button onClick={() => setOpen(true)}>reopen</button>
        <TypedConfirmDialog
          open={open()}
          onOpenChange={setOpen}
          title="Reset everything"
          consequences={["All settings are lost.", "Nothing can be undone."]}
          phrase="Reset Me"
          confirmLabel="Reset now"
          pending={extra.pending}
          onConfirm={onConfirm}
        />
      </>
    ));
    return { onConfirm, setOpen };
  }

  it("keeps confirm disabled until the typed text matches exactly", () => {
    const { onConfirm } = setup();
    const confirm = screen.getByRole("button", { name: "Reset now" });
    const field = screen.getByRole("textbox");
    expect(confirm).toBeDisabled();
    fireEvent.input(field, { target: { value: "reset me" } });
    expect(confirm).toBeDisabled();
    fireEvent.input(field, { target: { value: "Reset Me " } });
    expect(confirm).toBeDisabled();
    fireEvent.input(field, { target: { value: "Reset Me" } });
    expect(confirm).not.toBeDisabled();
    fireEvent.click(confirm);
    expect(onConfirm).toHaveBeenCalledTimes(1);
  });

  it("disables confirm while pending even when matching", () => {
    setup({ pending: true });
    fireEvent.input(screen.getByRole("textbox"), { target: { value: "Reset Me" } });
    expect(screen.getByRole("button", { name: "Reset now" })).toBeDisabled();
  });

  it("resets the input each time it opens", async () => {
    const { setOpen } = setup();
    fireEvent.input(screen.getByRole("textbox"), { target: { value: "Reset Me" } });
    setOpen(false);
    await Promise.resolve();
    setOpen(true);
    expect((await screen.findByRole("textbox")) as HTMLInputElement).toHaveValue("");
    expect(screen.getByRole("button", { name: "Reset now" })).toBeDisabled();
  });

  it("lists consequences in the dialog's description", () => {
    setup();
    const dialog = screen.getByRole("dialog");
    const describedBy = dialog.getAttribute("aria-describedby");
    expect(describedBy).toBeTruthy();
    const description = document.getElementById(describedBy!)!;
    expect(description).toHaveTextContent("All settings are lost.");
    expect(description).toHaveTextContent("Nothing can be undone.");
    expect(description.querySelectorAll("li")).toHaveLength(2);
  });
});
