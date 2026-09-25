import {
  createEffect,
  createMemo,
  createResource,
  createSignal,
  createUniqueId,
  For,
  lazy,
  Match,
  on,
  onCleanup,
  Show,
  Suspense,
  Switch,
} from "solid-js";
import { Button, Chip, Combobox, SeverityMarker, TextField, Timeline, type TimelineItem } from "../design-system";
import { isCommandError } from "../ipc/client";
import { slicing } from "../slicing/slicing-store";
import {
  createProject,
  loadRevisions,
  onRevisionCreated,
  reportLibraryError,
  setModelProjects,
  updateModel,
} from "../library/library-store";
import type {
  Inspection,
  ModelRecord,
  ModelSourceRevisionRecord,
  ModelSourceRevisionSummary,
  ProjectRecord,
} from "../library/types";
import {
  formatBytes,
  formatLabel,
  originLabel,
  shortHash,
  sourceStateMarker,
} from "./library-presentation";
import styles from "./ModelDetailsPanel.module.css";
import { InspectorPlaceholder } from "./InspectorPlaceholder";
import { ModelThumbnail } from "./ModelGrid";

// The 3D inspector (and the viewport it builds on) loads on first use, to
// keep it out of the main chunk.
const ModelPlateInspector = lazy(async () => ({ default: (await import("./ModelPlateInspector")).ModelPlateInspector }));
// So do the Slice Revision views (P5 D21).
const SliceRevisionList = lazy(async () => ({ default: (await import("./SliceRevisionList")).SliceRevisionList }));
const SliceRevisionReview = lazy(async () => ({ default: (await import("./SliceRevisionReview")).SliceRevisionReview }));
const GcodeFactsDialog = lazy(async () => ({ default: (await import("./GcodeFactsDialog")).GcodeFactsDialog }));

export interface ModelDetailsPanelProps {
  model: ModelRecord;
  projects: ProjectRecord[];
  /** **Locate source…** (D16), offered while a linked source isn't `ok`. */
  onLocateSource: (modelId: string) => void;
  /** **Convert to managed** (D5), offered for every linked Model. The
   *  caller confirms first. */
  onConvertToManaged: (modelId: string) => void;
  /** **Delete…** (D18). The caller confirms first. */
  onDelete: (modelId: string) => void;
  /** **Prepare…** (P5 D19), offered for STL and 3MF Models: opens the
   *  Preparation workspace. The Library's one way in. */
  onPrepare?: (modelId: string) => void;
  /** A Slice Revision to show the review of when the panel opens (a
   *  finished slice's **Open the Slice Revision**). */
  openRevisionId?: string | null;
  /** Called once `openRevisionId` has been acted on. */
  onRevisionOpened?: () => void;
  /** A new non-zero value moves focus to **Add to Project…**. */
  focusRequest?: number;
  /** Called once `focusRequest` has been acted on, so a remount doesn't
   *  replay it. */
  onFocusHandled?: () => void;
}

const NEW_PROJECT = "__new-project__";

interface ProjectOption {
  id: string;
  name: string;
}

function errorMessage(error: unknown): string {
  return isCommandError(error) ? error.message : "The change could not be saved.";
}

/** D19's details panel: name, Projects, format, storage and source state,
 *  the current revision, the format's own findings, the Slice Revisions
 *  (P5 D21), the revision history, and the actions: **Prepare…** for STL
 *  and 3MF, **Create Slice Revision…** for G-code. A Slice Revision's
 *  review replaces the details in the same dock until **Model details**. */
export function ModelDetailsPanel(props: ModelDetailsPanelProps) {
  const [reviewing, setReviewing] = createSignal<string | null>(null);
  createEffect(on(() => props.openRevisionId, (id) => {
    if (!id) return;
    setReviewing(id);
    props.onRevisionOpened?.();
  }));
  let panel: HTMLDivElement | undefined;
  /** Back from a review: focus returns to that revision's entry, or to the
   *  section if it was deleted. */
  const endReview = (revisionId: string) => {
    setReviewing(null);
    const stillThere = slicing.revision(revisionId) !== undefined;
    // The list mounts again (and its chunk may still be arriving), so its
    // entry can take a moment to appear.
    const refocus = (tries: number) => {
      const entry = panel?.querySelector<HTMLElement>(`[data-revision-open="${revisionId}"]`);
      const heading = panel?.querySelector<HTMLElement>("[data-slice-revisions-heading]");
      if (entry) entry.focus();
      else if (stillThere && tries > 0) setTimeout(() => refocus(tries - 1), 16);
      else heading?.focus();
    };
    queueMicrotask(() => refocus(30));
  };

  return (
    <div ref={panel} class={styles.dock}>
      <Show when={reviewing()} keyed fallback={<ModelDetails {...props} onReview={setReviewing} />}>
        {(revisionId) => (
          <Suspense fallback={<p class={styles.note} role="status">Loading the Slice Revision…</p>}>
            <SliceRevisionReview
              sliceRevisionId={revisionId}
              onBack={() => endReview(revisionId)}
              onDeleted={() => endReview(revisionId)}
            />
          </Suspense>
        )}
      </Show>
    </div>
  );
}

function ModelDetails(props: ModelDetailsPanelProps & { onReview: (sliceRevisionId: string) => void }) {
  // The newest revision this panel has heard of: the record's current one,
  // or a `library.revision.created` that got here first. Both arrive for
  // one capture, in either order, and name the same revision.
  const [announced, setAnnounced] = createSignal<ModelSourceRevisionSummary | undefined>();
  // Only a linked-source capture says "Updated from source"; Locate and
  // Add as a new revision are the user's own doing.
  const [captured, setCaptured] = createSignal<ModelSourceRevisionSummary | undefined>();
  onCleanup(onRevisionCreated((revision) => {
    if (revision.modelId !== props.model.id) return;
    setAnnounced(revision);
    setCaptured(revision.origin === "linkedChange" ? revision : undefined);
  }));
  const newestRevisionId = createMemo(() => {
    const current = props.model.currentRevision;
    const heard = announced();
    return heard && heard.sequence > current.sequence ? heard.id : current.id;
  });

  // Keyed by the newest revision too, so a newly captured revision
  // refreshes the history (once), but a rename or membership change
  // doesn't.
  const [revisions] = createResource(
    () => `${props.model.id}\n${newestRevisionId()}`,
    () => loadRevisions(props.model.id),
  );
  const historyId = createUniqueId();
  const sliceRevisionsId = createUniqueId();
  // The facts dialog loads on first use, then stays mounted (closed).
  const [factsOpen, setFactsOpen] = createSignal(false);
  const [factsMounted, setFactsMounted] = createSignal(false);
  const gcodeClaims = () => {
    const inspection = readyInspection();
    return inspection?.format === "gcode" ? inspection.claims : [];
  };
  // Reading an errored resource throws, so check the error first.
  const currentInspection = (): Inspection | undefined =>
    revisions.error ? undefined : revisions()?.find((revision) => revision.id === props.model.currentRevision.id)?.inspection;
  // The inspector sits in a Suspense boundary (for its lazy chunk), so its
  // props must not read a pending resource, which would suspend it again.
  const readyInspection = (): Inspection | undefined => {
    if (revisions.state !== "ready" && revisions.state !== "refreshing") return undefined;
    return revisions.latest?.find((revision) => revision.id === props.model.currentRevision.id)?.inspection;
  };
  const sourcePlates = () => {
    const inspection = readyInspection();
    return inspection?.format === "3mf" ? inspection.plates : [];
  };

  return (
    <div class={styles.panel}>
      <Show
        when={props.model.format !== "gcode"}
        fallback={<div class={styles.thumbnail}><ModelThumbnail model={props.model} /></div>}
      >
        <Suspense fallback={<InspectorPlaceholder>Loading the 3D view…</InspectorPlaceholder>}>
          <ModelPlateInspector model={props.model} plates={sourcePlates()} />
        </Suspense>
      </Show>
      <NameField model={props.model} />
      <ProjectMembership
        model={props.model}
        projects={props.projects}
        focusRequest={props.focusRequest}
        onFocusHandled={props.onFocusHandled}
      />
      <dl class={styles.facts}>
        <dt>Format</dt>
        <dd>{formatLabel(props.model.format)}</dd>
        <dt>Storage</dt>
        <dd>
          <StorageFacts model={props.model} />
        </dd>
        <dt>Current revision</dt>
        <dd>
          <RevisionSummary model={props.model} />
          <Show when={captured()}>
            {(revision) => (
              <p class={styles.captured} role="status">Updated from source · revision {revision().sequence}</p>
            )}
          </Show>
        </dd>
      </dl>
      <Switch>
        <Match when={props.model.format === "gcode"}>
          <GcodeFindings inspection={currentInspection()} />
        </Match>
        <Match when={props.model.format === "3mf"}>
          <ThreeMfFindings inspection={currentInspection()} />
        </Match>
      </Switch>
      <section class={styles.section} aria-labelledby={sliceRevisionsId}>
        <h3 id={sliceRevisionsId} class={styles.heading} tabIndex={-1} data-slice-revisions-heading>Slice Revisions</h3>
        <Suspense fallback={<p class={styles.note}>Loading the Slice Revisions…</p>}>
          <SliceRevisionList
            modelId={props.model.id}
            external={props.model.format === "gcode"}
            onOpen={props.onReview}
          />
        </Suspense>
      </section>
      <section class={styles.section} aria-labelledby={historyId}>
        <h3 id={historyId} class={styles.heading}>Revisions</h3>
        <Show when={!revisions.error} fallback={<p class={styles.note}>The revision history could not load.</p>}>
          <Timeline label="Revision history" items={timelineItems(revisions() ?? [])} />
        </Show>
      </section>
      <div class={styles.actions}>
        <Show when={props.onPrepare && props.model.format !== "gcode"}>
          <Button variant="primary" onClick={() => props.onPrepare?.(props.model.id)}>Prepare…</Button>
        </Show>
        <Show when={props.model.format === "gcode"}>
          <Button
            variant="primary"
            onClick={() => {
              setFactsMounted(true);
              setFactsOpen(true);
            }}
          >
            Create Slice Revision…
          </Button>
        </Show>
        <Show when={props.model.link}>
          {(link) => (
            <>
              <Show when={link().state !== "ok"}>
                <Button variant="secondary" onClick={() => props.onLocateSource(props.model.id)}>
                  Locate source…
                </Button>
              </Show>
              <Button variant="secondary" onClick={() => props.onConvertToManaged(props.model.id)}>
                Convert to managed
              </Button>
            </>
          )}
        </Show>
        <Button variant="secondary" onClick={() => props.onDelete(props.model.id)}>Delete…</Button>
      </div>
      <Show when={factsMounted()}>
        <Suspense>
          <GcodeFactsDialog
            open={factsOpen()}
            onOpenChange={setFactsOpen}
            sourceRevisionId={props.model.currentRevision.id}
            sourceRevisionSequence={props.model.currentRevision.sequence}
            claims={gcodeClaims()}
            onCreated={(record) => {
              setFactsOpen(false);
              props.onReview(record.id);
            }}
          />
        </Suspense>
      </Show>
    </div>
  );
}

function timelineItems(revisions: ModelSourceRevisionRecord[]): TimelineItem[] {
  return revisions.map((revision) => ({
    id: revision.id,
    at: revision.capturedAt,
    title: `Revision ${revision.sequence} · ${originLabel(revision.origin)}`,
    detail: `${shortHash(revision.sha256)} · ${formatBytes(revision.sizeBytes)}`,
  }));
}

/** Saves on blur or Enter, never per keystroke. Escape puts the saved name
 *  back. A rejection (for example a blank name) shows under the field. */
function NameField(props: { model: ModelRecord }) {
  const [draft, setDraft] = createSignal(props.model.name);
  const [error, setError] = createSignal<string | undefined>();
  const [saving, setSaving] = createSignal(false);

  // Reset only when the saved name (or the Model) actually changes, so an
  // unrelated update to the record doesn't discard an edit in progress.
  const saved = createMemo(() => `${props.model.id}\n${props.model.name}`);
  createEffect(on(saved, () => {
    setDraft(props.model.name);
    setError(undefined);
  }));

  const commit = () => {
    const next = draft().trim();
    if (saving() || next === props.model.name) return;
    setSaving(true);
    setError(undefined);
    updateModel(props.model.id, { name: next })
      .catch((failure: unknown) => setError(errorMessage(failure)))
      .finally(() => setSaving(false));
  };

  return (
    <div
      onFocusOut={commit}
      onKeyDown={(event) => {
        if (event.key === "Enter") {
          event.preventDefault();
          commit();
        } else if (event.key === "Escape") {
          setDraft(props.model.name);
          setError(undefined);
        }
      }}
    >
      <TextField label="Name" value={draft()} onChange={setDraft} error={error()} />
    </div>
  );
}

function ProjectMembership(props: {
  model: ModelRecord;
  projects: ProjectRecord[];
  focusRequest?: number;
  onFocusHandled?: () => void;
}) {
  // Remounting the picker after each choice clears its input, so it acts
  // as an action rather than holding a value.
  const [pickerKey, setPickerKey] = createSignal(1);
  const [creating, setCreating] = createSignal(false);
  let picker: HTMLDivElement | undefined;

  createEffect(on(() => props.focusRequest, (request) => {
    if (!request) return;
    picker?.querySelector<HTMLInputElement>("input")?.focus();
    props.onFocusHandled?.();
  }));

  const members = () =>
    props.model.projectIds.flatMap((id) => props.projects.filter((project) => project.id === id));
  const options = (): ProjectOption[] => [
    ...props.projects
      .filter((project) => !props.model.projectIds.includes(project.id))
      .sort((a, b) => a.name.localeCompare(b.name, undefined, { sensitivity: "base" }))
      .map((project) => ({ id: project.id, name: project.name })),
    { id: NEW_PROJECT, name: "New Project…" },
  ];

  const change = (modelId: string, add: string[], remove: string[]) => {
    setModelProjects(modelId, { add, remove }).catch(reportLibraryError);
  };

  const pick = (option: ProjectOption) => {
    setPickerKey((key) => key + 1);
    if (option.id === NEW_PROJECT) setCreating(true);
    else change(props.model.id, [option.id], []);
  };

  return (
    <div class={styles.membership}>
      <div class={styles.chips} role="group" aria-label="Projects">
        <For each={members()} fallback={<span class={styles.unfiled}>Unfiled</span>}>
          {(project) => (
            <Chip onRemove={() => change(props.model.id, [], [project.id])}>{project.name}</Chip>
          )}
        </For>
      </div>
      <div ref={picker}>
        <Show when={pickerKey()} keyed>
          {(_key) => <Combobox<ProjectOption>
            label="Add to Project…"
            placeholder="Choose a Project"
            options={options()}
            optionValue={(option) => option.id}
            optionLabel={(option) => option.name}
            onChange={pick}
          />}
        </Show>
      </div>
      <Show when={creating()}>
        <NewProjectForm modelId={props.model.id} onDone={() => setCreating(false)} />
      </Show>
    </div>
  );
}

/** **New Project…**: create the Project, then add this Model to it. A
 *  rejected name shows inline; a failed add goes to the Library banner. */
function NewProjectForm(props: { modelId: string; onDone: () => void }) {
  const [name, setName] = createSignal("");
  const [error, setError] = createSignal<string | undefined>();
  const [busy, setBusy] = createSignal(false);

  const create = async () => {
    setBusy(true);
    setError(undefined);
    try {
      const project = await createProject(name());
      props.onDone();
      await setModelProjects(props.modelId, { add: [project.id], remove: [] }).catch(reportLibraryError);
    } catch (failure) {
      setError(errorMessage(failure));
    } finally {
      setBusy(false);
    }
  };

  return (
    <form
      class={styles.newProject}
      onSubmit={(event) => {
        event.preventDefault();
        void create();
      }}
    >
      <TextField label="New Project name" value={name()} onChange={setName} error={error()} />
      <div class={styles.formActions}>
        <Button type="submit" variant="primary" size="sm" disabled={busy()}>Create and add</Button>
        <Button type="button" variant="ghost" size="sm" onClick={props.onDone}>Cancel</Button>
      </div>
    </form>
  );
}

function StorageFacts(props: { model: ModelRecord }) {
  return (
    <Show when={props.model.link} fallback={<span>Managed: farm3d keeps its own copy.</span>}>
      {(link) => (
        <div class={styles.storage}>
          <span>Linked</span>
          <SeverityMarker {...sourceStateMarker(link().state)} />
          <code class={styles.path}>{link().path}</code>
          <Show when={link().watchMode === "polling"}>
            <p class={styles.note}>This folder can't be watched, so farm3d checks the file every 10 seconds.</p>
          </Show>
        </div>
      )}
    </Show>
  );
}

function RevisionSummary(props: { model: ModelRecord }) {
  const revision = () => props.model.currentRevision;
  const findings = () => {
    const summary = revision().summary;
    switch (summary.format) {
      case "stl":
        return `${summary.triangleCount.toLocaleString()} triangles, millimetres assumed`;
      case "3mf":
        return `${summary.objectCount} objects, ${summary.plateCount} plates, ${summary.triangleCount.toLocaleString()} triangles`;
      case "gcode":
        return [
          `${summary.lineCount.toLocaleString()} lines`,
          summary.producer ? `from ${summary.producer.name} ${summary.producer.version}` : null,
        ].filter(Boolean).join(", ");
    }
  };
  return (
    <span>
      Revision {revision().sequence} · {revision().sourceFileName} · {formatBytes(revision().sizeBytes)}
      <br />
      <span class={styles.muted}>{findings()}</span>
    </span>
  );
}

function GcodeFindings(props: { inspection: Inspection | undefined }) {
  const headingId = createUniqueId();
  const claims = () => (props.inspection?.format === "gcode" ? props.inspection.claims : []);
  return (
    <>
      <p class={styles.note}>
        Pre-sliced G-code. Create a Slice Revision to confirm which Printer and material it is for.
      </p>
      <section class={styles.section} aria-labelledby={headingId}>
        <h3 id={headingId} class={styles.heading}>What the file says (not verified)</h3>
        <Show when={claims().length > 0} fallback={<p class={styles.note}>No claims found.</p>}>
          <dl class={styles.claims}>
            <For each={claims()}>
              {(claim) => (
                <>
                  <dt>{claim.key}</dt>
                  <dd>
                    {claim.value} <span class={styles.muted}>(line {claim.line.toLocaleString()})</span>
                  </dd>
                </>
              )}
            </For>
          </dl>
        </Show>
      </section>
    </>
  );
}

function ThreeMfFindings(props: { inspection: Inspection | undefined }) {
  const headingId = createUniqueId();
  const unsupported = () => (props.inspection?.format === "3mf" ? props.inspection.unsupported : []);
  return (
    <Show when={unsupported().length > 0}>
      <section class={styles.section} aria-labelledby={headingId}>
        <h3 id={headingId} class={styles.heading}>Not used by farm3d</h3>
        <p class={styles.note}>Kept in the stored file. farm3d won't use these when slicing.</p>
        <ul class={styles.unused}>
          <For each={unsupported()}>
            {(entry) => (
              <li>
                <code class={styles.path}>{entry.part}</code>
                <span class={styles.muted}>{entry.detail}</span>
              </li>
            )}
          </For>
        </ul>
      </section>
    </Show>
  );
}
