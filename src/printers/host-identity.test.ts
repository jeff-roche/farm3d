import { describe, expect, it } from "vitest";
import vectors from "../../src-tauri/tests/fixtures/host-identity.json";
import { canonicalHostIdentity } from "./host-identity";

interface Vector {
  host: string;
  port: number;
  expected: string | null;
}

describe("canonicalHostIdentity", () => {
  it("matches every Rust fixture vector (shared D2 conformance suite)", () => {
    for (const vector of vectors as Vector[]) {
      expect(canonicalHostIdentity(vector.host, vector.port)).toBe(vector.expected);
    }
  });
});
