import { render, screen } from "@solidjs/testing-library";
import { afterEach, describe, expect, it } from "vitest";
import { Timeline } from "./Timeline";
import styles from "./Timeline.module.css";

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
    const { container } = render(() => (
      <Timeline
        label="Empty detail"
        items={[{ id: "1", at: "2026-09-20T10:00:00Z", title: "Created" }]}
      />
    ));

    expect(screen.getByText("Created")).toBeInTheDocument();
    expect(container.querySelector(`.${styles.detail}`)).toBeNull();
  });

  it("never emits an 'undefined' class on the default marker", () => {
    const { container } = render(() => (
      <Timeline
        label="Markers"
        items={[
          { id: "1", at: "2026-09-20T10:00:00Z", title: "Default marker" },
          { id: "2", at: "2026-09-20T10:00:00Z", title: "Muted marker", marker: "muted" },
        ]}
      />
    ));

    const markers = container.querySelectorAll(`.${styles.marker}`);
    expect(markers).toHaveLength(2);
    for (const marker of markers) {
      expect(marker.className.split(/\s+/)).not.toContain("undefined");
    }
    expect(markers[0]?.classList.contains(styles.muted)).toBe(false);
    expect(markers[1]?.classList.contains(styles.muted)).toBe(true);
  });
});
