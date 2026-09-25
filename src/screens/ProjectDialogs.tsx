import { createMemo, createSignal, Show, type JSX } from "solid-js";
import { Button, Dialog, TextField } from "../design-system";
import { isCommandError } from "../ipc/client";
import { createProject, deleteProject, library, renameProject } from "../library/library-store";
import type { ProjectRecord } from "../library/types";
import styles from "./ProjectDialogs.module.css";

/** A failure split by where it shows: a `VALIDATION` on `name` under the
 *  name field, anything else as the dialog's own alert. */
interface NameFailure {
  field?: string;
  form?: string;
}

function nameFailure(error: unknown, fallback: string): NameFailure {
  if (!isCommandError(error)) return { form: fallback };
  if (error.code === "VALIDATION" && (error.details?.fieldPath ?? "name") === "name") return { field: error.message };
  return { form: error.message };
}

/** The shared body of **New Project** and **Rename…**: one name field,
 *  saved on submit. */
function ProjectNameDialog(props: {
  title: string;
  submitLabel: string;
  initialName: string;
  save: (name: string) => Promise<void>;
  onClose: () => void;
  returnFocus?: () => HTMLElement | null | undefined;
}) {
  const [name, setName] = createSignal(props.initialName);
  const [failure, setFailure] = createSignal<NameFailure>({});
  const [busy, setBusy] = createSignal(false);
  // No closing mid-request (see DeleteProjectDialog).
  const requestClose = () => {
    if (!busy()) props.onClose();
  };

  const submit: JSX.EventHandler<HTMLFormElement, SubmitEvent> = (event) => {
    event.preventDefault();
    if (busy()) return;
    setBusy(true);
    setFailure({});
    props
      .save(name())
      .catch((error: unknown) => setFailure(nameFailure(error, "The Project could not be saved.")))
      .finally(() => setBusy(false));
  };

  return (
    <Dialog
      title={props.title}
      open
      onOpenChange={(open) => {
        if (!open) requestClose();
      }}
      returnFocus={props.returnFocus}
    >
      <form class={styles.body} onSubmit={submit}>
        <TextField label="Name" value={name()} onChange={setName} error={failure().field} />
        <Show when={failure().form}>{(message) => <p class={styles.error} role="alert">{message()}</p>}</Show>
        <div class={styles.actions}>
          <Button type="button" variant="ghost" disabled={busy()} onClick={requestClose}>Cancel</Button>
          <Button type="submit" variant="primary" disabled={busy()}>{props.submitLabel}</Button>
        </div>
      </form>
    </Dialog>
  );
}

export function CreateProjectDialog(props: {
  onClose: () => void;
  /** The Project was created; the caller closes the dialog. */
  onCreated: (project: ProjectRecord) => void;
  returnFocus?: () => HTMLElement | null | undefined;
}) {
  return (
    <ProjectNameDialog
      title="New Project"
      submitLabel="Create Project"
      initialName=""
      save={async (name) => props.onCreated(await createProject(name))}
      onClose={props.onClose}
      returnFocus={props.returnFocus}
    />
  );
}

export function RenameProjectDialog(props: { project: ProjectRecord; onClose: () => void }) {
  return (
    <ProjectNameDialog
      title="Rename Project"
      submitLabel="Rename"
      initialName={props.project.name}
      save={async (name) => {
        await renameProject(props.project.id, name);
        props.onClose();
      }}
      onClose={props.onClose}
    />
  );
}

/** D18: "Delete Brackets? Its 3 Models stay in the Library. 1 of them will
 *  become Unfiled." Counted from the store's `projectIds`: a member whose
 *  only Project this is becomes Unfiled. */
function deleteProjectText(name: string, members: number, becomeUnfiled: number): string {
  if (members === 0) return `Delete ${name}? It has no Models.`;
  const stay = members === 1
    ? `Its 1 Model stays in the Library.`
    : `Its ${members} Models stay in the Library.`;
  if (becomeUnfiled === 0) return `Delete ${name}? ${stay}`;
  const unfiled = members === 1 ? "It will become Unfiled." : `${becomeUnfiled} of them will become Unfiled.`;
  return `Delete ${name}? ${stay} ${unfiled}`;
}

export function DeleteProjectDialog(props: {
  project: ProjectRecord;
  onClose: () => void;
  /** The Project is gone; the caller closes the dialog. */
  onDeleted: (projectId: string) => void;
}) {
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  // No closing mid-request: a late result would act on whatever dialog
  // the workspace had opened in the meantime.
  const requestClose = () => {
    if (!busy()) props.onClose();
  };
  const text = createMemo(() => {
    const members = library.models().filter((model) => model.projectIds.includes(props.project.id));
    const onlyHere = members.filter((model) => model.projectIds.length === 1).length;
    return deleteProjectText(props.project.name, members.length, onlyHere);
  });

  const confirm = async () => {
    if (busy()) return;
    setBusy(true);
    setError(null);
    // Read before awaiting: once the Project leaves the store, the caller
    // may already have unmounted this dialog, and `props.project` with it.
    const projectId = props.project.id;
    try {
      await deleteProject(projectId);
      props.onDeleted(projectId);
    } catch (failure) {
      setError(isCommandError(failure) ? failure.message : "The Project could not be deleted.");
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog
      title="Delete Project"
      open
      onOpenChange={(open) => {
        if (!open) requestClose();
      }}
    >
      <div class={styles.body}>
        <p class={styles.text}>{text()}</p>
        <Show when={error()}>{(message) => <p class={styles.error} role="alert">{message()}</p>}</Show>
        <div class={styles.actions}>
          <Button variant="ghost" disabled={busy()} onClick={requestClose}>Cancel</Button>
          <Button variant="danger" disabled={busy()} onClick={() => void confirm()}>Delete Project</Button>
        </div>
      </div>
    </Dialog>
  );
}
