import { describe, expect, it } from "vitest";
import { estimatedUseText, settlementPreviewText } from "./settlement";

describe("estimatedUseText", () => {
  it("formats estimatedUseMg the same way Spool weights format (one decimal by default)", () => {
    expect(estimatedUseText(15_000)).toBe("15.0 g");
  });

  it("accepts a whole-gram precision for a table", () => {
    expect(estimatedUseText(15_000, 0)).toBe("15 g");
  });
});

describe("settlementPreviewText", () => {
  it("formats a present preview", () => {
    expect(settlementPreviewText({ estimatedUseMg: 15_000 })).toBe("15.0 g");
  });

  it("is null when there is no preview (settlement isn't pending/deferred, ruling R4)", () => {
    expect(settlementPreviewText(null)).toBeNull();
  });
});
