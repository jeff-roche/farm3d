import { IconDots } from "@tabler/icons-solidjs";
import { DropdownMenu, type DropdownMenuEntry } from "../design-system";
import { reportLibraryError, setModelProjects } from "../library/library-store";
import type { LibraryView } from "../library/saved-views";
import type { ModelRecord, ProjectRecord } from "../library/types";
import styles from "./ModelActionsMenu.module.css";

export interface ModelActionsMenuProps {
  model: ModelRecord;
  view: LibraryView;
  projects: ProjectRecord[];
  onAddToProject: (modelId: string) => void;
}

/** D19: the menu on a grid card or list row. **Remove from <Project>**
 *  appears only inside that Project's view, and removes only that one
 *  membership; a failure goes to the Library banner, since a menu has no
 *  inline error of its own. */
export function ModelActionsMenu(props: ModelActionsMenuProps) {
  const items = (): DropdownMenuEntry[] => {
    const entries: DropdownMenuEntry[] = [
      { label: "Add to Project…", onSelect: () => props.onAddToProject(props.model.id) },
    ];
    const view = props.view;
    if (view.kind === "project" && props.model.projectIds.includes(view.id)) {
      const name = props.projects.find((project) => project.id === view.id)?.name ?? "this Project";
      entries.push({
        label: `Remove from ${name}`,
        onSelect: () => {
          setModelProjects(props.model.id, { add: [], remove: [view.id] }).catch(reportLibraryError);
        },
      });
    }
    return entries;
  };

  return (
    <DropdownMenu
      trigger={
        <span class={styles.trigger} aria-label={`Actions for ${props.model.name}`}>
          <IconDots size={16} aria-hidden="true" />
        </span>
      }
      items={items()}
    />
  );
}
