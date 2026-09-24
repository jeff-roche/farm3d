import { fireEvent, render, screen } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ActivityBar } from "./ActivityBar";

afterEach(() => {
  document.body.innerHTML = "";
});

describe("ActivityBar", () => {
  it("exposes the Monitor, Library, Spools, and Settings controls with the active destination identified, and no Queue button", async () => {
    const onSelect = vi.fn();
    render(() => <ActivityBar active="monitor" onSelect={onSelect} />);

    const monitor = screen.getByRole("button", { name: "Monitor" });
    expect(monitor).toHaveAttribute("aria-current", "page");
    expect(screen.getByRole("button", { name: "Library" })).not.toHaveAttribute("aria-current");
    expect(screen.getByRole("button", { name: "Spools" })).not.toHaveAttribute("aria-current");
    expect(screen.getByLabelText("Settings")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /queue/i })).not.toBeInTheDocument();

    await fireEvent.click(screen.getByRole("button", { name: "Library" }));
    expect(onSelect).toHaveBeenCalledWith("library");

    await fireEvent.click(screen.getByRole("button", { name: "Spools" }));
    expect(onSelect).toHaveBeenCalledWith("spools");
  });

  it("marks Spools current when active, and shows no badge with a zero low count", () => {
    render(() => <ActivityBar active="spools" onSelect={vi.fn()} lowSpoolCount={0} />);
    expect(screen.getByRole("button", { name: "Spools" })).toHaveAttribute("aria-current", "page");
  });

  it("shows a badge equal to the number of low Spools", () => {
    render(() => <ActivityBar active="monitor" onSelect={vi.fn()} lowSpoolCount={3} />);
    expect(screen.getByRole("button", { name: "Spools (3 low)" })).toBeInTheDocument();
    expect(screen.getByText("3")).toBeInTheDocument();
  });
});
