import { cleanup, render, screen } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import { buildWebLibraryFixture } from "../library/web-fixtures";
import { ModelDetailsPanel } from "./ModelDetailsPanel";

vi.mock("../library/library-store", async () => (await import("../library/library-store-mock")).libraryStoreMock);
vi.mock("../slicing/slicing-store", async () => (await import("../slicing/slicing-store-mock")).slicingStoreMock);

// Holds the lazily loaded inspector's chunk until the test lets it arrive.
const chunk = vi.hoisted(() => {
  let release!: () => void;
  const arrived = new Promise<void>((resolve) => {
    release = resolve;
  });
  return { arrived, release };
});
vi.mock("./ModelPlateInspector", async () => {
  await chunk.arrived;
  return { ModelPlateInspector: () => <p>Inspector loaded</p> };
});

afterEach(cleanup);

describe("ModelDetailsPanel's lazy 3D inspector", () => {
  it("holds its place with the loading placeholder until the chunk arrives", async () => {
    const model = buildWebLibraryFixture(new Date("2026-09-24T12:00:00Z")).models.find((m) => m.id === "mdl-web-bracket")!;
    render(() => (
      <ModelDetailsPanel
        model={model}
        projects={[]}
        onLocateSource={vi.fn()}
        onConvertToManaged={vi.fn()}
        onDelete={vi.fn()}
      />
    ));

    const placeholder = screen.getByRole("status");
    expect(placeholder).toHaveTextContent("Loading the 3D view…");
    // The rest of the panel doesn't wait for the inspector.
    expect(screen.getByRole("textbox", { name: "Name" })).toBeInTheDocument();
    expect(screen.queryByText("Inspector loaded")).toBeNull();

    chunk.release();
    expect(await screen.findByText("Inspector loaded")).toBeInTheDocument();
    expect(screen.queryByText("Loading the 3D view…")).toBeNull();
  });
});
