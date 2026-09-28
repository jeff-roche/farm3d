import { createRoot, createSignal } from "solid-js";
import { afterEach, describe, expect, it, vi } from "vitest";

const cameraStoreMock = vi.hoisted(() => ({ snapshotImageUrl: vi.fn() }));
vi.mock("./camera-store", () => cameraStoreMock);

import { useSnapshotImage, type SnapshotImageSource } from "./useSnapshotImage";

async function flush(): Promise<void> {
  for (let i = 0; i < 10; i += 1) await Promise.resolve();
}

afterEach(() => {
  cameraStoreMock.snapshotImageUrl.mockReset();
});

describe("useSnapshotImage", () => {
  it("clears the previous source's error when the source changes", async () => {
    const failure = { contractVersion: 1, code: "EVIDENCE_PRUNED", message: "gone", recovery: [], retryable: false };
    cameraStoreMock.snapshotImageUrl.mockImplementation((id: string) =>
      id === "snp-bad" ? Promise.reject(failure) : Promise.resolve(`data:image/png;base64,${id}`));

    const [source, setSource] = createSignal<SnapshotImageSource>({ id: "snp-bad", prunedAt: null });
    let image!: ReturnType<typeof useSnapshotImage>;
    const dispose = createRoot((d) => {
      image = useSnapshotImage(source);
      return d;
    });
    await flush();
    expect(image.error()).toBe(failure);
    expect(image.url()).toBeNull();

    // A good image after a failed one: its error is gone.
    setSource({ id: "snp-good", prunedAt: null });
    expect(image.error()).toBeNull();
    await flush();
    expect(image.url()).toBe("data:image/png;base64,snp-good");
    expect(image.error()).toBeNull();

    // A pruned row after a failed one: no fetch, and no stale error.
    setSource({ id: "snp-bad", prunedAt: null });
    await flush();
    expect(image.error()).toBe(failure);
    setSource({ id: "snp-pruned", prunedAt: "2026-09-25T00:00:00Z" });
    expect(image.error()).toBeNull();
    expect(image.url()).toBeNull();
    expect(cameraStoreMock.snapshotImageUrl).not.toHaveBeenCalledWith("snp-pruned");
    dispose();
  });
});
