import { fireEvent, render, screen } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ActivityBar } from "./ActivityBar";

afterEach(() => {
  document.body.innerHTML = "";
});

describe("ActivityBar", () => {
  it("exposes the Monitor, Queue, Library, Spools, and Settings controls with the active destination identified", async () => {
    const onSelect = vi.fn();
    render(() => <ActivityBar active="monitor" onSelect={onSelect} />);

    const monitor = screen.getByRole("button", { name: "Monitor" });
    expect(monitor).toHaveAttribute("aria-current", "page");
    expect(screen.getByRole("button", { name: "Queue" })).not.toHaveAttribute("aria-current");
    expect(screen.getByRole("button", { name: "Library" })).not.toHaveAttribute("aria-current");
    expect(screen.getByRole("button", { name: "Spools" })).not.toHaveAttribute("aria-current");
    expect(screen.getByLabelText("Settings")).toBeInTheDocument();
    // The umbrella spec's rail order: Monitor, Queue, Library, Spools.
    expect(screen.getAllByRole("button").slice(0, 4).map((button) => button.getAttribute("aria-label")))
      .toEqual(["Monitor", "Queue", "Library", "Spools"]);

    await fireEvent.click(screen.getByRole("button", { name: "Queue" }));
    expect(onSelect).toHaveBeenCalledWith("queue");

    await fireEvent.click(screen.getByRole("button", { name: "Library" }));
    expect(onSelect).toHaveBeenCalledWith("library");

    await fireEvent.click(screen.getByRole("button", { name: "Spools" }));
    expect(onSelect).toHaveBeenCalledWith("spools");
  });

  it("marks Spools current when active, and shows no badge with a zero attention count", () => {
    render(() => <ActivityBar active="spools" onSelect={vi.fn()} attentionSpoolCount={0} />);
    expect(screen.getByRole("button", { name: "Spools" })).toHaveAttribute("aria-current", "page");
  });

  it("shows a badge equal to the number of Spools needing attention (low or reconciliation)", () => {
    render(() => <ActivityBar active="monitor" onSelect={vi.fn()} attentionSpoolCount={3} />);
    expect(screen.getByRole("button", { name: "Spools (3 need attention)" })).toBeInTheDocument();
    expect(screen.getByText("3")).toBeInTheDocument();
  });

  it("marks Monitor current when active, and shows no badge with nothing actionable", () => {
    render(() => <ActivityBar active="monitor" onSelect={vi.fn()} attentionActionableCount={0} />);
    const monitor = screen.getByRole("button", { name: "Monitor" });
    expect(monitor).toHaveAttribute("aria-current", "page");
    expect(monitor.parentElement).not.toHaveTextContent(/\d/);
  });

  it("badges Monitor with the same actionable count as the Attention trigger", () => {
    render(() => <ActivityBar active="queue" onSelect={vi.fn()} attentionActionableCount={4} />);
    const monitor = screen.getByRole("button", { name: "Monitor (4 need attention)" });
    expect(monitor.parentElement).toHaveTextContent("4");
  });

  it("marks Queue current when active, and shows no Queue badge with nothing needing attention", () => {
    render(() => <ActivityBar active="queue" onSelect={vi.fn()} queueAttentionCount={0} />);
    const queue = screen.getByRole("button", { name: "Queue" });
    expect(queue).toHaveAttribute("aria-current", "page");
    expect(queue.parentElement).not.toHaveTextContent(/\d/);
  });

  it("badges the Queue with the entries and requirements needing attention", () => {
    render(() => <ActivityBar active="monitor" onSelect={vi.fn()} queueAttentionCount={5} />);
    const queue = screen.getByRole("button", { name: "Queue (5 need attention)" });
    expect(queue.parentElement).toHaveTextContent("5");
  });
});
