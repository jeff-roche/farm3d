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

  it("announces with the caller's announce text when given, e.g. an absolute position", async () => {
    const onMove = vi.fn();
    render(() => (
      <ReorderHandle
        label="Bracket v3"
        index={0}
        count={2}
        onMove={onMove}
        announce={(from, to) => `Moved Bracket v3 from ${from} to position ${to === 1 ? 5 : 2} of 7`}
      />
    ));
    await fireEvent.keyDown(getHandle("Bracket v3"), { key: "ArrowDown", altKey: true });
    expect(document.querySelector('[aria-live="polite"]')?.textContent).toBe("Moved Bracket v3 from 0 to position 5 of 7");
  });

  it("announces a committed pointer-drag move in the live region", async () => {
    const onMove = vi.fn();
    render(() => <ReorderHandle label="Bracket v3" index={1} count={5} onMove={onMove} />);
    const handle = getHandle("Bracket v3");
    vi.spyOn(handle, "getBoundingClientRect").mockReturnValue({ height: 32 } as DOMRect);

    await fireEvent.pointerDown(handle, { pointerId: 1, clientY: 100, button: 0 });
    await fireEvent.pointerMove(window, { pointerId: 1, clientY: 170 });
    await fireEvent.pointerUp(window, { pointerId: 1, clientY: 170 });

    const live = document.querySelector('[aria-live="polite"]');
    expect(live?.textContent).toBe("Moved Bracket v3 to position 4 of 5");
  });

  it("ignores a pointerdown while a drag is already active", async () => {
    const onMove = vi.fn();
    render(() => <ReorderHandle label="Bracket v3" index={1} count={5} onMove={onMove} />);
    const handle = getHandle("Bracket v3");
    vi.spyOn(handle, "getBoundingClientRect").mockReturnValue({ height: 32 } as DOMRect);

    await fireEvent.pointerDown(handle, { pointerId: 1, clientY: 100, button: 0 });
    await fireEvent.pointerMove(window, { pointerId: 1, clientY: 170 }); // -> target index 3

    // A second pointerdown mid-drag must be ignored, not restart the drag
    // (which would reset the start position) or add a duplicate listener
    // set (which would fire onMove twice for one pointerup).
    await fireEvent.pointerDown(handle, { pointerId: 2, clientY: 0, button: 0 });

    await fireEvent.pointerUp(window, { pointerId: 1, clientY: 170 });

    expect(onMove).toHaveBeenCalledTimes(1);
    expect(onMove).toHaveBeenCalledWith(1, 3);
  });

  // No `[data-reorder-row]` ancestor in any of these three — they exercise
  // the fallback path (the handle's own measured height as a uniform unit).
  it("a pointer drag across two rows emits one move (fallback: no data-reorder-row ancestor)", async () => {
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

  it("clamps a pointer drag that overshoots the list (fallback path)", async () => {
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

  describe("row geometry (data-reorder-row)", () => {
    function ThreeRowList(props: { onMove: (from: number, to: number) => void }) {
      return (
        <div>
          <div data-reorder-row>
            <ReorderHandle label="Row A" index={0} count={3} onMove={props.onMove} />
          </div>
          <div data-reorder-row>Row B</div>
          <div data-reorder-row>Row C</div>
        </div>
      );
    }

    // Rows of different heights: mid 10, mid 60, mid 120.
    function mockThreeRowRects() {
      const rows = Array.from(document.querySelectorAll("[data-reorder-row]"));
      const specs = [
        { top: 0, height: 20 },
        { top: 20, height: 80 },
        { top: 100, height: 40 },
      ];
      rows.forEach((row, i) => {
        vi.spyOn(row, "getBoundingClientRect").mockReturnValue(specs[i] as DOMRect);
      });
    }

    it("lands on the middle row by midpoint, not a uniform step", async () => {
      const onMove = vi.fn();
      render(() => <ThreeRowList onMove={onMove} />);
      mockThreeRowRects();
      const handle = getHandle("Row A");

      // Between row A's midpoint (10) and row B's midpoint (60): one row passed.
      await fireEvent.pointerDown(handle, { pointerId: 1, clientY: 5, button: 0 });
      await fireEvent.pointerMove(window, { pointerId: 1, clientY: 30 });
      await fireEvent.pointerUp(window, { pointerId: 1, clientY: 30 });

      expect(onMove).toHaveBeenCalledTimes(1);
      expect(onMove).toHaveBeenCalledWith(0, 1);
    });

    it("clamps to the last row when the pointer overshoots", async () => {
      const onMove = vi.fn();
      render(() => <ThreeRowList onMove={onMove} />);
      mockThreeRowRects();
      const handle = getHandle("Row A");

      await fireEvent.pointerDown(handle, { pointerId: 1, clientY: 5, button: 0 });
      await fireEvent.pointerMove(window, { pointerId: 1, clientY: 500 });
      await fireEvent.pointerUp(window, { pointerId: 1, clientY: 500 });

      expect(onMove).toHaveBeenCalledTimes(1);
      expect(onMove).toHaveBeenCalledWith(0, 2);
    });
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

  it("disabled blocks keyboard and pointer moves on the handle", async () => {
    const onMove = vi.fn();
    render(() => <ReorderHandle label="Bracket v3" index={1} count={4} onMove={onMove} disabled />);
    const handle = getHandle("Bracket v3");

    await fireEvent.keyDown(handle, { key: "ArrowDown", altKey: true });
    await fireEvent.pointerDown(handle, { pointerId: 1, clientY: 0, button: 0 });
    await fireEvent.pointerMove(window, { pointerId: 1, clientY: 500 });
    await fireEvent.pointerUp(window, { pointerId: 1, clientY: 500 });

    expect(onMove).not.toHaveBeenCalled();
  });

  it("disabled keeps the menu trigger present but inert, not hidden", async () => {
    render(() => <ReorderHandle label="Bracket v3" index={1} count={4} onMove={vi.fn()} disabled />);

    // Present, not removed from the tree.
    const trigger = screen.getByLabelText("Move Bracket v3");
    expect(trigger).toBeInTheDocument();
    expect(trigger.closest('[aria-disabled="true"]')).not.toBeNull();

    // Doesn't open on pointerdown.
    await fireEvent.pointerDown(trigger, { pointerType: "mouse", button: 0 });
    expect(screen.queryByText("Move to top")).not.toBeInTheDocument();
  });
});
