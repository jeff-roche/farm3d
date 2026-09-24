import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { model, project } from "./test-records";
import type { LibraryEvent, LibraryEventType, ModelRecord, ProjectRecord } from "./types";

const tauriMock = vi.hoisted(() => ({ isTauri: vi.fn(), invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => tauriMock);

const eventMock = vi.hoisted(() => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => eventMock);

const STREAM = "stream-library";

type Handler = (event: { payload: unknown }) => void;
type Responder = (args: Record<string, unknown>) => unknown;

let handler: Handler;
let unlisten: ReturnType<typeof vi.fn>;
let calls: string[];
let responders: Record<string, Responder>;

function snapshot(sequence: number, projects: ProjectRecord[] = [], models: ModelRecord[] = []) {
  return { streamId: STREAM, snapshotSequence: sequence, projects, models };
}

const CONTENT_INFO = { blobCount: 2, totalBytes: 2048, pendingCleanupCount: 0 };

beforeEach(() => {
  vi.resetModules();
  tauriMock.isTauri.mockReset();
  tauriMock.invoke.mockReset();
  eventMock.listen.mockReset();
  tauriMock.isTauri.mockReturnValue(true);
  calls = [];
  handler = () => {};
  unlisten = vi.fn();
  responders = {
    list_library: () => snapshot(0),
    library_content_info: () => CONTENT_INFO,
  };
  eventMock.listen.mockImplementation((name: string, cb: Handler) => {
    calls.push(`listen:${name}`);
    handler = cb;
    return Promise.resolve(unlisten);
  });
  tauriMock.invoke.mockImplementation((name: string, args: Record<string, unknown>) => {
    calls.push(`invoke:${name}`);
    const respond = responders[name];
    if (!respond) return Promise.reject(new Error(`unexpected command ${name}`));
    try {
      const data = respond(args);
      return data instanceof Promise
        ? data.then((value) => ({ contractVersion: 1, data: value }))
        : Promise.resolve({ contractVersion: 1, data });
    } catch (error) {
      return Promise.reject(error);
    }
  });
});

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

function envelope(sequence: number, type: LibraryEventType | string, payload: unknown, subjectId = "x", streamId = STREAM) {
  return {
    contractVersion: 1,
    streamId,
    sequence,
    eventId: `evt-${sequence}`,
    occurredAt: "2026-09-24T00:00:00Z",
    type,
    subject: { kind: type.split(".")[1] ?? "x", id: subjectId },
    payload,
  } as LibraryEvent;
}

function emit(event: LibraryEvent): void {
  handler({ payload: event });
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

async function flush(): Promise<void> {
  for (let i = 0; i < 20; i += 1) await Promise.resolve();
}

function commandError(code: string, message: string, extra: Record<string, unknown> = {}) {
  return { contractVersion: 1, code, message, recovery: [], retryable: false, ...extra };
}

async function startedStore() {
  const store = await import("./library-store");
  await store.startLibrary();
  return store;
}

describe("startLibrary (desktop)", () => {
  it("1. listens before backfilling, drops buffered events at or below snapshotSequence, and replays later ones in order", async () => {
    const backfill = deferred<unknown>();
    responders.list_library = () => backfill.promise;
    const { startLibrary, library } = await import("./library-store");
    const started = startLibrary();
    await flush();
    expect(library.status()).toBe("loading");
    expect(library.syncState()).toBe("syncing");
    expect(calls).toContain("invoke:list_library");
    expect(calls.indexOf("listen:farm3d-event-v1")).toBeLessThan(calls.indexOf("invoke:list_library"));

    // Arrive while backfilling, out of order: 7 before 6, and 5 is covered.
    emit(envelope(7, "library.model.changed", model({ id: "mdl-a", revision: 3, name: "Seven" }), "mdl-a"));
    emit(envelope(5, "library.project.changed", project({ id: "prj-stale", name: "Stale" }), "prj-stale"));
    emit(envelope(6, "library.model.changed", model({ id: "mdl-a", revision: 2, name: "Six" }), "mdl-a"));
    backfill.resolve(snapshot(5, [], [model({ id: "mdl-a", revision: 1, name: "Snapshot" })]));
    await started;

    expect(library.status()).toBe("ready");
    expect(library.syncState()).toBe("current");
    expect(library.projects()).toEqual([]);
    expect(library.models().map((m) => [m.id, m.revision, m.name])).toEqual([["mdl-a", 3, "Seven"]]);

    emit(envelope(8, "library.project.changed", project({ id: "prj-new", name: "New" }), "prj-new"));
    expect(library.projects().map((p) => p.id)).toEqual(["prj-new"]);
  });

  it("loads the content totals after the snapshot", async () => {
    const { library } = await startedStore();
    await flush();
    expect(library.contentInfo()).toEqual(CONTENT_INFO);
  });

  it("2. ignores a library.model.changed whose revision is not newer than the one held", async () => {
    responders.list_library = () => snapshot(1, [], [model({ id: "mdl-a", revision: 3, name: "Held" })]);
    const { library } = await startedStore();

    emit(envelope(2, "library.model.changed", model({ id: "mdl-a", revision: 3, name: "Same revision" }), "mdl-a"));
    emit(envelope(3, "library.model.changed", model({ id: "mdl-a", revision: 2, name: "Older" }), "mdl-a"));
    expect(library.models()[0].name).toBe("Held");

    emit(envelope(4, "library.model.changed", model({ id: "mdl-a", revision: 4, name: "Newer" }), "mdl-a"));
    expect(library.models()[0].name).toBe("Newer");
    expect(library.syncState()).toBe("current");
  });

  it("applies removals by subject id", async () => {
    responders.list_library = () => snapshot(1, [project({ id: "prj-b" })], [model({ id: "mdl-a" })]);
    const { library } = await startedStore();
    emit(envelope(2, "library.model.removed", {}, "mdl-a"));
    emit(envelope(3, "library.project.removed", {}, "prj-b"));
    expect(library.models()).toEqual([]);
    expect(library.projects()).toEqual([]);
  });

  it("3. a sequence gap triggers a new backfill and marks the store uncertain until it settles", async () => {
    const { library } = await startedStore();
    expect(tauriMock.invoke.mock.calls.filter(([name]) => name === "list_library")).toHaveLength(1);

    const rebackfill = deferred<unknown>();
    responders.list_library = () => rebackfill.promise;
    emit(envelope(1, "library.project.changed", project({ id: "prj-1" }), "prj-1"));
    emit(envelope(3, "library.project.changed", project({ id: "prj-3" }), "prj-3"));
    await flush();

    expect(library.syncState()).toBe("uncertain");
    expect(tauriMock.invoke.mock.calls.filter(([name]) => name === "list_library")).toHaveLength(2);

    rebackfill.resolve(snapshot(3, [project({ id: "prj-1" }), project({ id: "prj-2" }), project({ id: "prj-3" })]));
    await flush();
    expect(library.syncState()).toBe("current");
    expect(library.projects().map((p) => p.id)).toEqual(["prj-1", "prj-2", "prj-3"]);
  });

  it("3b. a failed resync keeps the store uncertain and retries with backoff", async () => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    const { library } = await startedStore();
    responders.list_library = () => { throw commandError("PERSISTENCE_UNAVAILABLE", "Storage is busy."); };
    emit(envelope(2, "library.project.changed", project(), "prj-1"));
    await flush();
    expect(library.syncState()).toBe("uncertain");
    const backfills = () => tauriMock.invoke.mock.calls.filter(([name]) => name === "list_library").length;
    expect(backfills()).toBe(2);

    await vi.advanceTimersByTimeAsync(999);
    expect(backfills()).toBe(2);
    await vi.advanceTimersByTimeAsync(1);
    expect(backfills()).toBe(3);
    await vi.advanceTimersByTimeAsync(2_000);
    expect(backfills()).toBe(4);

    responders.list_library = () => snapshot(2, [project()]);
    await vi.advanceTimersByTimeAsync(4_000);
    expect(backfills()).toBe(5);
    expect(library.syncState()).toBe("current");
    expect(library.projects().map((p) => p.id)).toEqual(["prj-1"]);
  });

  it("4. ignores every event on the shared channel that is not library.*", async () => {
    const { library } = await startedStore();
    const foreign = [
      envelope(1, "printer.status.changed", { type: "changed" }, "prn-1", "stream-status"),
      envelope(9, "printer.status.removed", { type: "removed" }, "prn-1", "stream-status"),
      envelope(4, "spool.changed", { type: "spoolChanged" }, "spl-1", "stream-inventory"),
      envelope(5, "printer.slots.changed", { type: "printerSlotsChanged" }, "prn-1", "stream-inventory"),
    ];
    for (const event of foreign) emit(event);
    await flush();

    expect(library.syncState()).toBe("current");
    expect(tauriMock.invoke.mock.calls.filter(([name]) => name === "list_library")).toHaveLength(1);
    // The Library's own sequence is untouched: its next event still applies.
    emit(envelope(1, "library.project.changed", project(), "prj-1"));
    expect(library.projects()).toHaveLength(1);
  });

  it("5. library.selection.dropped calls onSelectionDropped handlers, and progress reaches onImportProgress", async () => {
    const { onSelectionDropped, onImportProgress } = await startedStore();
    const dropped = vi.fn();
    const progress = vi.fn();
    const stopDropped = onSelectionDropped(dropped);
    onImportProgress(progress);

    const summary = { selectionId: "sel-1", purpose: "import", files: [{ fileIndex: 0, fileName: "cube.stl", sizeBytes: 684 }] };
    emit(envelope(1, "library.selection.dropped", summary, "sel-1"));
    emit(envelope(2, "library.import.progress", { fileIndex: 0, bytesDone: 10, bytesTotal: 684 }, "sel-1"));
    expect(dropped).toHaveBeenCalledWith(summary);
    expect(progress).toHaveBeenCalledWith("sel-1", { fileIndex: 0, bytesDone: 10, bytesTotal: 684 });

    stopDropped();
    emit(envelope(3, "library.selection.dropped", { ...summary, selectionId: "sel-2" }, "sel-2"));
    expect(dropped).toHaveBeenCalledTimes(1);
  });

  it("5c. library.revision.created reaches onRevisionCreated handlers in stream order", async () => {
    const { onRevisionCreated } = await startedStore();
    const created = vi.fn();
    const stop = onRevisionCreated(created);
    const revision = { ...model({ id: "mdl-a" }).currentRevision, id: "msr-2", sequence: 2, origin: "linkedChange" };
    emit(envelope(1, "library.revision.created", revision, "msr-2"));
    expect(created).toHaveBeenCalledWith(revision);

    stop();
    emit(envelope(2, "library.revision.created", { ...revision, id: "msr-3", sequence: 3 }, "msr-3"));
    expect(created).toHaveBeenCalledTimes(1);
  });

  it("5b. a drop that arrives while the snapshot is loading is delivered at once, not held back", async () => {
    const backfill = deferred<unknown>();
    responders.list_library = () => backfill.promise;
    const { startLibrary, onSelectionDropped } = await import("./library-store");
    const dropped = vi.fn();
    onSelectionDropped(dropped);
    const started = startLibrary();
    await flush();

    const summary = { selectionId: "sel-1", purpose: "import", files: [] };
    emit(envelope(4, "library.selection.dropped", summary, "sel-1"));
    expect(dropped).toHaveBeenCalledWith(summary);
    backfill.resolve(snapshot(4));
    await started;
    expect(dropped).toHaveBeenCalledTimes(1);
  });

  it("is idempotent: starting again disposes the previous listener, and the returned disposer unlistens", async () => {
    const first = vi.fn();
    const second = vi.fn();
    const { startLibrary } = await import("./library-store");
    unlisten = first;
    await startLibrary();
    unlisten = second;
    const dispose = await startLibrary();
    expect(first).toHaveBeenCalledOnce();
    expect(second).not.toHaveBeenCalled();
    dispose();
    expect(second).toHaveBeenCalledOnce();
  });

  it("reports a failed first load as an error with a user-safe message, then recovers on retry", async () => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    responders.list_library = () => { throw new Error("socket path /home/me leaked"); };
    const { library } = await startedStore();
    expect(library.status()).toBe("error");
    expect(library.error()).toBe("The Library could not load.");
    expect(library.syncState()).toBe("uncertain");

    responders.list_library = () => snapshot(0, [project()]);
    await vi.advanceTimersByTimeAsync(1_000);
    expect(library.status()).toBe("ready");
    expect(library.error()).toBeNull();
    expect(library.projects()).toHaveLength(1);
  });
});

describe("mutations (desktop)", () => {
  it("6. createProject settles from the command result", async () => {
    const created = project({ id: "prj-new", name: "Fixtures" });
    responders.create_project = () => ({ project: created });
    const { createProject, library } = await startedStore();
    await expect(createProject("Fixtures")).resolves.toEqual(created);
    expect(tauriMock.invoke).toHaveBeenCalledWith("create_project", { contractVersion: 1, name: "Fixtures" });
    expect(library.projects()).toEqual([created]);
  });

  it("6b. createProject rejects with the CommandError on VALIDATION and does not raise the banner", async () => {
    const error = commandError("VALIDATION", "A Project with this name already exists.", { details: { fieldPath: "name" } });
    responders.create_project = () => { throw error; };
    const { createProject, library } = await startedStore();
    await expect(createProject("Brackets")).rejects.toEqual(error);
    expect(library.projects()).toEqual([]);
    expect(library.error()).toBeNull();
  });

  it("an older command result never overwrites a newer event", async () => {
    responders.list_library = () => snapshot(0, [project({ id: "prj-1", revision: 1, name: "Old" })]);
    const rename = deferred<unknown>();
    responders.rename_project = () => rename.promise;
    const { renameProject, library } = await startedStore();
    const pending = renameProject("prj-1", "Mine");
    await flush();
    expect(tauriMock.invoke).toHaveBeenCalledWith("rename_project", { contractVersion: 1, id: "prj-1", expectedRevision: 1, name: "Mine" });
    emit(envelope(1, "library.project.changed", project({ id: "prj-1", revision: 3, name: "Theirs" }), "prj-1"));
    rename.resolve({ project: project({ id: "prj-1", revision: 2, name: "Mine" }) });
    await pending;
    expect(library.projects()[0].name).toBe("Theirs");
  });

  it("a command result for a Model already removed by an event does not bring it back", async () => {
    responders.list_library = () => snapshot(0, [], [model({ id: "mdl-a", revision: 1 })]);
    const update = deferred<unknown>();
    responders.update_model = () => update.promise;
    const { updateModel, library } = await startedStore();
    const pending = updateModel("mdl-a", { name: "Renamed" });
    await flush();
    emit(envelope(1, "library.model.removed", {}, "mdl-a"));
    update.resolve({ model: model({ id: "mdl-a", revision: 2, name: "Renamed" }), warnings: [] });
    await pending;
    expect(library.models()).toEqual([]);
  });

  it("refreshLibrary backfills again, for the workspace's Refresh", async () => {
    const { refreshLibrary, library } = await startedStore();
    responders.list_library = () => snapshot(0, [project({ id: "prj-fresh" })]);
    refreshLibrary();
    await flush();
    expect(tauriMock.invoke.mock.calls.filter(([name]) => name === "list_library")).toHaveLength(2);
    expect(library.projects().map((p) => p.id)).toEqual(["prj-fresh"]);
  });

  it("a CONFLICT rejects and refreshes the Library so a retry uses the fresh revision", async () => {
    responders.list_library = () => snapshot(0, [], [model({ id: "mdl-a", revision: 1 })]);
    const { updateModel, library } = await startedStore();
    responders.update_model = () => { throw commandError("CONFLICT", "This Model changed."); };
    responders.list_library = () => snapshot(0, [], [model({ id: "mdl-a", revision: 5, name: "Theirs" })]);
    await expect(updateModel("mdl-a", { name: "Mine" })).rejects.toMatchObject({ code: "CONFLICT" });
    await flush();
    expect(library.models()[0]).toMatchObject({ revision: 5, name: "Theirs" });
  });

  it("7. checkSources throttles to one call per 30 s unless forced", async () => {
    let now = 1_000_000;
    vi.spyOn(Date, "now").mockImplementation(() => now);
    responders.list_library = () => snapshot(0, [], [model({ id: "mdl-a", revision: 1 })]);
    const changed = model({ id: "mdl-a", revision: 2, storageMode: "linked", link: { path: "/m/a.stl", state: "missing", checkedAt: null, watchMode: "watching" } });
    responders.check_linked_sources = () => [changed];
    const { checkSources, library } = await startedStore();
    const checks = () => tauriMock.invoke.mock.calls.filter(([name]) => name === "check_linked_sources");

    await checkSources();
    expect(checks()).toHaveLength(1);
    expect(checks()[0][1]).toEqual({ contractVersion: 1 });
    expect(library.models()[0].link?.state).toBe("missing");

    now += 29_999;
    await checkSources();
    expect(checks()).toHaveLength(1);

    await checkSources(["mdl-a"], { force: true });
    expect(checks()).toHaveLength(2);
    expect(checks()[1][1]).toEqual({ contractVersion: 1, modelIds: ["mdl-a"] });

    now += 30_000;
    await checkSources();
    expect(checks()).toHaveLength(3);
  });

  it("8. setModelProjects settles projectIds from the result, and library.project.changed updates modelCount", async () => {
    responders.list_library = () => snapshot(0, [project({ id: "prj-b", modelCount: 0 })], [model({ id: "mdl-a", revision: 4 })]);
    responders.set_model_projects = () => ({ model: model({ id: "mdl-a", revision: 5, projectIds: ["prj-b"] }), warnings: [] });
    const { setModelProjects, library } = await startedStore();

    await setModelProjects("mdl-a", { add: ["prj-b"], remove: [] });
    expect(tauriMock.invoke).toHaveBeenCalledWith("set_model_projects", {
      contractVersion: 1, modelId: "mdl-a", expectedRevision: 4, add: ["prj-b"], remove: [],
    });
    expect(library.models()[0].projectIds).toEqual(["prj-b"]);

    // Membership never bumps a Project's revision: the count arrives at the same revision.
    emit(envelope(1, "library.project.changed", project({ id: "prj-b", modelCount: 1 }), "prj-b"));
    expect(library.projects()[0].modelCount).toBe(1);
  });

  it("deleteProject drops the Project and its id from the affected Models, never the Models", async () => {
    responders.list_library = () => snapshot(0, [project({ id: "prj-b" })], [
      model({ id: "mdl-a", projectIds: ["prj-b"] }),
      model({ id: "mdl-b", projectIds: [] }),
    ]);
    responders.delete_project = () => ({ deletedId: "prj-b", affectedModelIds: ["mdl-a"], nowUnfiledModelIds: ["mdl-a"] });
    const { deleteProject, library } = await startedStore();
    await deleteProject("prj-b");
    expect(tauriMock.invoke).toHaveBeenCalledWith("delete_project", { contractVersion: 1, id: "prj-b", expectedRevision: 1 });
    expect(library.projects()).toEqual([]);
    expect(library.models().map((m) => [m.id, m.projectIds])).toEqual([["mdl-a", []], ["mdl-b", []]]);
  });

  it("deleteModel removes the Model", async () => {
    responders.list_library = () => snapshot(0, [], [model({ id: "mdl-a", revision: 2 })]);
    responders.delete_model = () => ({ deletedId: "mdl-a", warnings: [] });
    const { deleteModel, library } = await startedStore();
    await deleteModel("mdl-a");
    expect(tauriMock.invoke).toHaveBeenCalledWith("delete_model", { contractVersion: 1, id: "mdl-a", expectedRevision: 2 });
    expect(library.models()).toEqual([]);
  });

  it("importModels sends one operationId, settles the imported Models, and returns the per-item outcomes", async () => {
    const imported = model({ id: "mdl-new" });
    const result = { items: [{ fileIndex: 0, outcome: "imported", model: imported, errors: [], warnings: [] }] };
    responders.import_models = () => result;
    const { importModels, library } = await startedStore();
    const items = [{ fileIndex: 0, name: "Cube", projectIds: [], storageMode: "managed" as const, acknowledgeUnsupported: false }];
    await expect(importModels("sel-1", items, "op-1")).resolves.toEqual(result);
    expect(tauriMock.invoke).toHaveBeenCalledWith("import_models", { contractVersion: 1, selectionId: "sel-1", operationId: "op-1", items });
    expect(library.models()).toEqual([imported]);
  });

  it("locateSource and convertToManaged send the held revision and settle the Model", async () => {
    responders.list_library = () => snapshot(0, [], [model({ id: "mdl-a", revision: 3 })]);
    responders.locate_linked_source = () => ({ model: model({ id: "mdl-a", revision: 4, name: "Located" }), warnings: [] });
    responders.convert_model_to_managed = () => ({ model: model({ id: "mdl-a", revision: 5, name: "Managed" }), warnings: [] });
    const { locateSource, convertToManaged, library } = await startedStore();

    await expect(locateSource("mdl-a", "sel-1", 0, false)).resolves.toMatchObject({ revision: 4 });
    expect(tauriMock.invoke).toHaveBeenCalledWith("locate_linked_source", {
      contractVersion: 1, modelId: "mdl-a", expectedRevision: 3, selectionId: "sel-1", fileIndex: 0, acceptDifferentContent: false,
    });
    await convertToManaged("mdl-a");
    expect(tauriMock.invoke).toHaveBeenCalledWith("convert_model_to_managed", { contractVersion: 1, modelId: "mdl-a", expectedRevision: 4 });
    expect(library.models()[0].name).toBe("Managed");
  });

  it("loadThumbnail returns a data: URL and caches it per revision", async () => {
    responders.get_revision_thumbnail = () => ({ mediaType: "image/png", width: 1, height: 1, dataBase64: "AAAA" });
    const { loadThumbnail } = await startedStore();
    await expect(loadThumbnail("msr-1")).resolves.toBe("data:image/png;base64,AAAA");
    await expect(loadThumbnail("msr-1")).resolves.toBe("data:image/png;base64,AAAA");
    expect(tauriMock.invoke.mock.calls.filter(([name]) => name === "get_revision_thumbnail")).toHaveLength(1);
  });

  it("reportLibraryError routes a non-dialog failure to the banner, and dismiss clears it", async () => {
    const { reportLibraryError, dismissLibraryError, library } = await startedStore();
    reportLibraryError(commandError("NOT_FOUND", "This Model no longer exists."));
    expect(library.error()).toBe("This Model no longer exists.");
    dismissLibraryError();
    expect(library.error()).toBeNull();
    reportLibraryError(new Error("/secret/path"));
    expect(library.error()).toBe("The operation could not be completed.");
  });
});

describe("web mode", () => {
  beforeEach(() => tauriMock.isTauri.mockReturnValue(false));

  it("9. loads the fixtures ready, without touching Tauri", async () => {
    const { library } = await startedStore();
    expect(library.status()).toBe("ready");
    expect(library.syncState()).toBe("current");
    expect(library.projects().map((p) => p.name).sort()).toEqual(["Brackets", "Calibration"]);
    expect(library.models()).toHaveLength(5);
    expect(library.contentInfo()).toBeNull();
    expect(eventMock.listen).not.toHaveBeenCalled();
    expect(tauriMock.invoke).not.toHaveBeenCalled();
  });

  it("9. createProject and setModelProjects work locally, keeping modelCount and name order", async () => {
    const { createProject, setModelProjects, library } = await startedStore();
    const created = await createProject("  Adapters ");
    expect(created).toMatchObject({ name: "Adapters", modelCount: 0, revision: 1 });

    const unfiled = library.models().find((m) => m.projectIds.length === 0)!;
    const brackets = library.projects().find((p) => p.name === "Brackets")!;
    await setModelProjects(unfiled.id, { add: [brackets.id, created.id], remove: [] });
    const after = library.models().find((m) => m.id === unfiled.id)!;
    expect(after.projectIds).toEqual([created.id, brackets.id]);
    expect(after.revision).toBe(unfiled.revision + 1);
    expect(library.projects().find((p) => p.id === created.id)!.modelCount).toBe(1);
    expect(library.projects().find((p) => p.id === brackets.id)!.modelCount).toBe(brackets.modelCount + 1);
  });

  it("9. local edits keep the backend's validation", async () => {
    const { createProject, setModelProjects, library } = await startedStore();
    await expect(createProject("   ")).rejects.toMatchObject({ code: "VALIDATION", details: { fieldPath: "name" } });
    await expect(createProject("brackets")).rejects.toMatchObject({ code: "VALIDATION", message: "A Project with this name already exists." });
    const m = library.models()[0];
    await expect(setModelProjects(m.id, { add: ["prj-nope"], remove: [] })).rejects.toMatchObject({ code: "NOT_FOUND" });
  });

  it("9. deleteProject leaves every Model in place and drops the Project from their projectIds", async () => {
    const { deleteProject, library } = await startedStore();
    const brackets = library.projects().find((p) => p.name === "Brackets")!;
    const before = library.models().map((m) => m.id);
    await deleteProject(brackets.id);
    expect(library.projects().map((p) => p.name)).toEqual(["Calibration"]);
    expect(library.models().map((m) => m.id)).toEqual(before);
    expect(library.models().every((m) => !m.projectIds.includes(brackets.id))).toBe(true);
  });

  it("9. pickFiles rejects with a CommandError that says importing needs the desktop app", async () => {
    const { pickFiles } = await startedStore();
    const { isCommandError } = await import("../ipc/client");
    const error = await pickFiles("import").catch((e: unknown) => e);
    expect(isCommandError(error)).toBe(true);
    expect(error).toMatchObject({ message: "Importing Models needs the desktop app." });
  });

  it("9. linked-source checks and the other desktop-only calls reject the same way", async () => {
    const { checkSources, inspectSelection, importModels, locateSource, convertToManaged } = await startedStore();
    const { isCommandError } = await import("../ipc/client");
    for (const attempt of [
      () => checkSources(undefined, { force: true }),
      () => inspectSelection("sel-1"),
      () => importModels("sel-1", []),
      () => locateSource("mdl-web-clip", "sel-1", 0, false),
      () => convertToManaged("mdl-web-clip"),
    ]) {
      const error = await attempt().catch((e: unknown) => e);
      expect(isCommandError(error)).toBe(true);
      expect((error as { message: string }).message).toMatch(/needs the desktop app\.$/);
    }
  });

  it("9. revisions and thumbnails come from the fixtures", async () => {
    const { loadRevisions, loadThumbnail, library } = await startedStore();
    const withThumbnail = library.models().find((m) => m.currentRevision.hasThumbnail)!;
    const history = await loadRevisions(withThumbnail.id);
    expect(history[0].id).toBe(withThumbnail.currentRevision.id);
    await expect(loadThumbnail(withThumbnail.currentRevision.id)).resolves.toMatch(/^data:image\/png;base64,/);
    await expect(loadThumbnail("msr-unknown")).resolves.toBeNull();
  });
});
