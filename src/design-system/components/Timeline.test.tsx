import { render, screen } from "@solidjs/testing-library";
import { afterEach, describe, expect, it } from "vitest";
import { Timeline } from "./Timeline";

afterEach(() => {
  document.body.innerHTML = "";
});

describe("Timeline", () => {
  it("renders an ordered list with a <time datetime> per item", () => {
    render(() => (
      <Timeline
        label="Spool history"
        items={[
          { id: "1", at: "2026-09-20T10:00:00Z", title: "Loaded into Atlas, slot 1" },
          {
            id: "2",
            at: "2026-09-21T08:30:00Z",
            title: "Recorded amount",
            detail: <span>842 g remaining</span>,
            marker: "muted",
          },
        ]}
      />
    ));

    const list = screen.getByRole("list", { name: "Spool history" });
    expect(list.tagName).toBe("OL");

    const times = list.querySelectorAll("time");
    expect(times).toHaveLength(2);
    expect(times[0]?.getAttribute("datetime")).toBe("2026-09-20T10:00:00Z");
    expect(times[1]?.getAttribute("datetime")).toBe("2026-09-21T08:30:00Z");

    expect(screen.getByText("Loaded into Atlas, slot 1")).toBeInTheDocument();
    expect(screen.getByText("842 g remaining")).toBeInTheDocument();
  });

  it("renders nothing under detail when omitted", () => {
    render(() => (
      <Timeline
        label="Empty detail"
        items={[{ id: "1", at: "2026-09-20T10:00:00Z", title: "Created" }]}
      />
    ));

    expect(screen.getByText("Created")).toBeInTheDocument();
  });
});
