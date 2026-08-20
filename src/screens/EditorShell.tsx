import { createMemo, createSignal, For, Show, type JSX } from "solid-js";
import { DropdownMenu, IconButton, TextField } from "../design-system";
import { GroundPlane } from "./GroundPlane";
import { ThemeMenu } from "./ThemeMenu";
import styles from "./EditorShell.module.css";

export type SceneObjectType = "terrain" | "field" | "building";

export interface SceneObject {
  id: string;
  name: string;
  type: SceneObjectType;
  children?: SceneObject[];
}

export interface EditorShellProps {
  farmName: string;
  scene: SceneObject[];
  onClose: () => void;
}

type ToolId = "select" | "move" | "terrain" | "plant";

const TOOLS: { id: ToolId; label: string; icon: () => JSX.Element }[] = [
  { id: "select", label: "Select", icon: SelectIcon },
  { id: "move", label: "Move", icon: MoveIcon },
  { id: "terrain", label: "Sculpt terrain", icon: TerrainIcon },
  { id: "plant", label: "Plant crop", icon: PlantIcon },
];

const TYPE_ABBR: Record<SceneObjectType, string> = {
  terrain: "T",
  field: "F",
  building: "B",
};

function flattenScene(objects: SceneObject[], depth = 0): { object: SceneObject; depth: number }[] {
  return objects.flatMap((object) => [
    { object, depth },
    ...flattenScene(object.children ?? [], depth + 1),
  ]);
}

export function EditorShell(props: EditorShellProps) {
  const [tool, setTool] = createSignal<ToolId>("select");
  const [selectedId, setSelectedId] = createSignal<string | null>(null);

  const rows = createMemo(() => flattenScene(props.scene));
  const selected = createMemo(() =>
    rows().find((row) => row.object.id === selectedId())?.object,
  );

  return (
    <div class={styles.shell}>
      <header class={styles.topBar}>
        <div class={styles.topBarLeading}>
          <DropdownMenu
            trigger={
              <span class={styles.appMenuTrigger} aria-label="farm3d menu">
                ≡
              </span>
            }
            items={[
              { label: "Close farm", onSelect: props.onClose },
              { type: "separator" },
              { label: "Save", onSelect: () => {}, disabled: true },
              { label: "Export...", onSelect: () => {}, disabled: true },
            ]}
          />
          <span class={styles.farmName}>{props.farmName}</span>
        </div>
        <ThemeMenu />
      </header>

      <nav class={styles.scenePanel} aria-label="Scene">
        <div class={styles.panelHeader}>Scene</div>
        <ul class={styles.sceneList}>
          <For each={rows()}>
            {(row) => (
              <li>
                <button
                  class={styles.sceneRow}
                  style={{ "padding-left": `${0.75 + row.depth * 0.9}rem` }}
                  classList={{ [styles.sceneRowSelected]: row.object.id === selectedId() }}
                  onClick={() => setSelectedId(row.object.id)}
                >
                  <span
                    class={styles.typeBadge}
                    classList={{ [styles[`typeBadge_${row.object.type}`]]: true }}
                  >
                    {TYPE_ABBR[row.object.type]}
                  </span>
                  <span class={styles.sceneRowName}>{row.object.name}</span>
                </button>
              </li>
            )}
          </For>
        </ul>
      </nav>

      <div class={styles.viewport}>
        <GroundPlane />

        <div class={styles.toolRail}>
          <For each={TOOLS}>
            {(t) => (
              <IconButton
                aria-label={t.label}
                active={tool() === t.id}
                onClick={() => setTool(t.id)}
              >
                {t.icon()}
              </IconButton>
            )}
          </For>
        </div>

        <Show when={rows().length <= 1}>
          <p class={styles.viewportHint}>
            Empty plot — pick a tool on the left to start building.
          </p>
        </Show>
      </div>

      <aside class={styles.propertiesPanel} aria-label="Properties">
        <div class={styles.panelHeader}>Properties</div>
        <Show
          when={selected()}
          fallback={<p class={styles.propertiesEmpty}>Select an object to edit its properties.</p>}
        >
          {(object) => (
            <div class={styles.propertiesBody}>
              <TextField label="Name" value={object().name} />
              <div class={styles.vectorRow}>
                <TextField label="X" type="number" defaultValue="0" />
                <TextField label="Y" type="number" defaultValue="0" />
                <TextField label="Z" type="number" defaultValue="0" />
              </div>
            </div>
          )}
        </Show>
      </aside>

      <footer class={styles.statusBar}>
        <span>0.0, 0.0, 0.0</span>
        <span>Plot 40 × 40 m</span>
        <span>Zoom 100%</span>
        <span class={styles.statusBarVersion}>farm3d 0.1.0</span>
      </footer>
    </div>
  );
}

function SelectIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 14 14" fill="none" aria-hidden="true">
      <path
        d="M2 1.5L2 12.5L5.2 9.6L7 13L8.6 12.2L6.9 8.8L11 8.6L2 1.5Z"
        fill="currentColor"
      />
    </svg>
  );
}

function MoveIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 14 14" fill="none" aria-hidden="true">
      <path
        d="M7 1L7 13M1 7L13 7M7 1L5 3M7 1L9 3M7 13L5 11M7 13L9 11M1 7L3 5M1 7L3 9M13 7L11 5M13 7L11 9"
        stroke="currentColor"
        stroke-width="1.3"
        stroke-linecap="round"
        stroke-linejoin="round"
      />
    </svg>
  );
}

function TerrainIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 14 14" fill="none" aria-hidden="true">
      <path
        d="M1 11L5 4.5L7.5 8L9.5 5L13 11Z"
        stroke="currentColor"
        stroke-width="1.3"
        stroke-linejoin="round"
      />
    </svg>
  );
}

function PlantIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 14 14" fill="none" aria-hidden="true">
      <path
        d="M7 13V7M7 7C7 7 3 7 3 3C7 3 7 7 7 7ZM7 7C7 7 11 6 11 2C7 2 7 7 7 7Z"
        stroke="currentColor"
        stroke-width="1.3"
        stroke-linejoin="round"
        stroke-linecap="round"
      />
    </svg>
  );
}
