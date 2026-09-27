import { fireEvent, render, screen } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ReorderHandle } from "./ReorderHandle";

afterEach(() => {
  document.body.innerHTML = "";
});

function getHandle(label: string) {
  return screen.getByRole("button", { name: `Reorder ${label}` });
}

describe("ReorderHandle", () => {
  it("renders a labeled drag handle and a companion menu trigger", () => {
    render(() => <ReorderHandle label="Bracket v3" index={1} count={4} onMove={vi.fn()} />);

    expect(getHandle("Bracket v3")).toBeInTheDocument();
    expect(screen.getByLabelText("Move Bracket v3")).toBeInTheDocument();
  });

  it("Alt+ArrowDown/ArrowUp move by one", async () => {
    const onMove = vi.fn();
    render(() => <ReorderHandle label="Bracket v3" index={1} count={4} onMove={onMove} />);
    const handle = getHandle("Bracket v3");

    await fireEvent.keyDown(handle, { key: "ArrowDown", altKey: true });
    expect(onMove).toHaveBeenCalledWith(1, 2);

    await fireEvent.keyDown(handle, { key: "ArrowUp", altKey: true });
    expect(onMove).toHaveBeenCalledWith(1, 0);

    expect(onMove).toHaveBeenCalledTimes(2);
  });

  it("Alt+Home/End move to the top and bottom", async () => {
    const onMove = vi.fn();
    render(() => <ReorderHandle label="Bracket v3" index={2} count={5} onMove={onMove} />);
    const handle = getHandle("Bracket v3");

    await fireEvent.keyDown(handle, { key: "Home", altKey: true });
    expect(onMove).toHaveBeenCalledWith(2, 0);

    await fireEvent.keyDown(handle, { key: "End", altKey: true });
    expect(onMove).toHaveBeenCalledWith(2, 4);

    expect(onMove).toHaveBeenCalledTimes(2);
  });

  it("clamps at the ends and emits nothing when already there", async () => {
    const onMove = vi.fn();
    render(() => <ReorderHandle label="Bracket v3" index={0} count={4} onMove={onMove} />);
    const handle = getHandle("Bracket v3");

    await fireEvent.keyDown(handle, { key: "ArrowUp", altKey: true });
    await fireEvent.keyDown(handle, { key: "Home", altKey: true });

    expect(onMove).not.toHaveBeenCalled();
  });

  it("ignores plain arrow keys without Alt", async () => {
    const onMove = vi.fn();
    render(() => <ReorderHandle label="Bracket v3" index={1} count={4} onMove={onMove} />);
    const handle = getHandle("Bracket v3");

    await fireEvent.keyDown(handle, { key: "ArrowDown" });

    expect(onMove).not.toHaveBeenCalled();
  });

  it("announces a committed move in a polite live region", async () => {
    const onMove = vi.fn();
    render(() => <ReorderHandle label="Bracket v3" index={1} count={4} onMove={onMove} />);
    const handle = getHandle("Bracket v3");

    await fireEvent.keyDown(handle, { key: "ArrowDown", altKey: true });

    const live = document.querySelector('[aria-live="polite"]');
    expect(live).not.toBeNull();
    expect(live?.textContent).toBe("Moved Bracket v3 to position 3 of 4");
  });

  it("a pointer drag across two rows emits one move", async () => {
    const onMove = vi.fn();
    render(() => <ReorderHandle label="Bracket v3" index={1} count={5} onMove={onMove} />);
    const handle = getHandle("Bracket v3");
    vi.spyOn(handle, "getBoundingClientRect").mockReturnValue({ height: 32 } as DOMRect);

    await fireEvent.pointerDown(handle, { pointerId: 1, clientY: 100, button: 0 });
    await fireEvent.pointerMove(window, { pointerId: 1, clientY: 170 });
    await fireEvent.pointerUp(window, { pointerId: 1, clientY: 170 });

    expect(onMove).toHaveBeenCalledTimes(1);
    expect(onMove).toHaveBeenCalledWith(1, 3);
  });

  it("clamps a pointer drag that overshoots the list", async () => {
    const onMove = vi.fn();
    render(() => <ReorderHandle label="Bracket v3" index={1} count={3} onMove={onMove} />);
    const handle = getHandle("Bracket v3");
    vi.spyOn(handle, "getBoundingClientRect").mockReturnValue({ height: 32 } as DOMRect);

    await fireEvent.pointerDown(handle, { pointerId: 1, clientY: 0, button: 0 });
    await fireEvent.pointerMove(window, { pointerId: 1, clientY: 500 });
    await fireEvent.pointerUp(window, { pointerId: 1, clientY: 500 });

    expect(onMove).toHaveBeenCalledTimes(1);
    expect(onMove).toHaveBeenCalledWith(1, 2);
  });

  it("Escape during a drag cancels and emits nothing", async () => {
    const onMove = vi.fn();
    render(() => <ReorderHandle label="Bracket v3" index={1} count={5} onMove={onMove} />);
    const handle = getHandle("Bracket v3");
    vi.spyOn(handle, "getBoundingClientRect").mockReturnValue({ height: 32 } as DOMRect);

    await fireEvent.pointerDown(handle, { pointerId: 1, clientY: 100, button: 0 });
    await fireEvent.pointerMove(window, { pointerId: 1, clientY: 170 });
    await fireEvent.keyDown(window, { key: "Escape" });
    await fireEvent.pointerUp(window, { pointerId: 1, clientY: 170 });

    expect(onMove).not.toHaveBeenCalled();
  });

  it("moves via the companion menu (pointerDown/pointerUp)", async () => {
    const onMove = vi.fn();
    render(() => <ReorderHandle label="Bracket v3" index={0} count={3} onMove={onMove} />);

    const trigger = screen.getByLabelText("Move Bracket v3");
    await fireEvent.pointerDown(trigger, { pointerType: "mouse", button: 0 });

    const moveDown = await screen.findByText("Move down");
    await fireEvent.pointerUp(moveDown, { button: 0 });
    expect(onMove).toHaveBeenCalledWith(0, 1);
  });

  it("disables Move to top/up at the start of the list", async () => {
    render(() => <ReorderHandle label="Bracket v3" index={0} count={3} onMove={vi.fn()} />);

    await fireEvent.pointerDown(screen.getByLabelText("Move Bracket v3"), { pointerType: "mouse", button: 0 });

    const moveToTop = await screen.findByText("Move to top");
    const moveUp = await screen.findByText("Move up");
    expect(moveToTop.closest('[role="menuitem"]')).toHaveAttribute("data-disabled");
    expect(moveUp.closest('[role="menuitem"]')).toHaveAttribute("data-disabled");
  });

  it("disables Move down/to bottom at the end of the list", async () => {
    render(() => <ReorderHandle label="Bracket v3" index={2} count={3} onMove={vi.fn()} />);

    await fireEvent.pointerDown(screen.getByLabelText("Move Bracket v3"), { pointerType: "mouse", button: 0 });

    const moveDown = await screen.findByText("Move down");
    const moveToBottom = await screen.findByText("Move to bottom");
    expect(moveDown.closest('[role="menuitem"]')).toHaveAttribute("data-disabled");
    expect(moveToBottom.closest('[role="menuitem"]')).toHaveAttribute("data-disabled");
  });

  it("disabled blocks keyboard and pointer moves and hides the menu", async () => {
    const onMove = vi.fn();
    render(() => <ReorderHandle label="Bracket v3" index={1} count={4} onMove={onMove} disabled />);
    const handle = getHandle("Bracket v3");

    await fireEvent.keyDown(handle, { key: "ArrowDown", altKey: true });
    await fireEvent.pointerDown(handle, { pointerId: 1, clientY: 0, button: 0 });
    await fireEvent.pointerMove(window, { pointerId: 1, clientY: 500 });
    await fireEvent.pointerUp(window, { pointerId: 1, clientY: 500 });

    expect(onMove).not.toHaveBeenCalled();
    expect(screen.queryByLabelText("Move Bracket v3")).not.toBeInTheDocument();
  });
});
