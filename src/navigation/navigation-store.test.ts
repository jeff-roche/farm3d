import { describe, expect, it } from "vitest";
import {
  createNavigationStore,
  parseNavigationTarget,
  serializeNavigationTarget,
  type NavigationTarget,
} from "./navigation-store";

describe("navigation identity", () => {
  const valid: NavigationTarget[] = [
    { version: 1, destination: "monitor" },
    { version: 1, destination: "monitor", selection: { kind: "printer", id: "Bay/α: 1" } },
    { version: 1, destination: "monitor", selection: { kind: "incident", id: "i-1" } },
    { version: 1, destination: "monitor", selection: { kind: "attention", id: "a-1" } },
    { version: 1, destination: "queue", selection: { kind: "job", id: "j-1" } },
    { version: 1, destination: "library", selection: { kind: "model", id: "m-1" } },
    { version: 1, destination: "library", selection: { kind: "project", id: "p-1" } },
    { version: 1, destination: "spools", selection: { kind: "spool", id: "s-1" } },
    { version: 1, destination: "settings" },
  ];

  it.each(valid)("round trips $destination/$selection.kind", (target) => {
    expect(parseNavigationTarget(serializeNavigationTarget(target))).toEqual(target);
  });

  it("uses canonical UTF-8 percent encoding", () => {
    expect(
      serializeNavigationTarget({
        version: 1,
        destination: "monitor",
        selection: { kind: "printer", id: "Bay/α: 1" },
      }),
    ).toBe("#nav=v1/monitor/printer/Bay%2F%CE%B1%3A%201");
  });

  it.each([
    "#nav=v2/monitor",
    "#nav=v1/settings/printer/x",
    "#nav=v1/queue/model/x",
    "#nav=v1/monitor/printer/%GG",
    "#nav=v1/library/model/",
    "#nav=v1/library/model/x/extra",
  ])("rejects invalid target %s", (value) => {
    expect(parseNavigationTarget(value)).toBeNull();
  });

  it("preserves an unavailable attempted identity", () => {
    const store = createNavigationStore();
    const target: NavigationTarget = {
      version: 1,
      destination: "monitor",
      selection: { kind: "printer", id: "missing" },
    };
    store.navigate(target, { availableDestinations: ["monitor", "library"], availableIds: [] });
    expect(store.target()).toEqual(target);
    expect(store.availability()).toBe("selectionUnavailable");
  });

  it("re-evaluates an attempted identity when durable records finish loading", () => {
    const store = createNavigationStore();
    const target: NavigationTarget = {
      version: 1,
      destination: "monitor",
      selection: { kind: "printer", id: "prn-1" },
    };
    store.navigate(target, { availableDestinations: ["monitor", "library"], availableIds: [] });
    store.navigate(store.target(), {
      availableDestinations: ["monitor", "library"],
      availableIds: ["prn-1"],
    });

    expect(store.target()).toEqual(target);
    expect(store.availability()).toBe("available");
  });
});
