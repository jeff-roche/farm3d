import { beforeEach, describe, expect, it } from "vitest";
import {
  hostOperationsStoreMock,
  loadWebHostOperationsFixture,
  resetHostOperationsStoreMock,
  setHostOperationsStoreState,
} from "./host-operations-store-mock";
import { hostOperation } from "./test-records";
import { WEB_HOST_OPS_PRINTER_FINISHED, WEB_HOST_OPS_STAGED_OPERATION } from "./web-fixtures";

describe("hostOperationsStoreMock", () => {
  beforeEach(() => resetHostOperationsStoreMock());

  it("mirrors every export of the real store's API", async () => {
    const real = await import("./host-operations-store");
    expect(Object.keys(hostOperationsStoreMock).sort()).toEqual(Object.keys(real).sort());
    expect(Object.keys(hostOperationsStoreMock.hostOperations).sort()).toEqual(Object.keys(real.hostOperations).sort());
  });

  it("serves web-fixtures.ts' Host Operations", () => {
    const fixture = loadWebHostOperationsFixture();
    expect(fixture.hostOperations.length).toBeGreaterThan(0);
    const { hostOperations } = hostOperationsStoreMock;
    expect(hostOperations.stagedFor(WEB_HOST_OPS_PRINTER_FINISHED).map((o) => o.id)).toEqual([WEB_HOST_OPS_STAGED_OPERATION]);
  });

  it("reacts when a test sets state directly", () => {
    setHostOperationsStoreState([hostOperation({ id: "hop-1", printerId: "prn-1", state: "uncertain" })]);
    expect(hostOperationsStoreMock.hostOperations.unresolvedFor("prn-1")?.id).toBe("hop-1");
    resetHostOperationsStoreMock();
    expect(hostOperationsStoreMock.hostOperations.operations()).toEqual([]);
  });
});
