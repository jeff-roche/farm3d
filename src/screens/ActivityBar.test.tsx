import { fireEvent, render, screen } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ActivityBar } from "./ActivityBar";

afterEach(() => {
  document.body.innerHTML = "";
});

describe("ActivityBar", () => {
  it("exposes only the Monitor, Library, and Settings controls with the active destination identified", async () => {
    const onSelect = vi.fn();
    render(() => <ActivityBar active="monitor" onSelect={onSelect} />);

    const monitor = screen.getByRole("button", { name: "Monitor" });
    expect(monitor).toHaveAttribute("aria-current", "page");
    expect(screen.getByRole("button", { name: "Library" })).not.toHaveAttribute("aria-current");
    expect(screen.getByLabelText("Settings")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /queue|spools/i })).not.toBeInTheDocument();

    await fireEvent.click(screen.getByRole("button", { name: "Library" }));
    expect(onSelect).toHaveBeenCalledWith("library");
  });
});
