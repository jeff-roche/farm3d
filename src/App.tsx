import { createSignal, Show } from "solid-js";
import { EditorShell, type SceneObject } from "./screens/EditorShell";
import { Welcome, type RecentFarm } from "./screens/Welcome";

const RECENT_FARMS: RecentFarm[] = [
  { name: "Willow Creek", editedLabel: "2 days ago" },
  { name: "North Field", editedLabel: "1 week ago" },
];

const DEFAULT_SCENE: SceneObject[] = [{ id: "terrain", name: "Terrain", type: "terrain" }];

const WILLOW_CREEK_SCENE: SceneObject[] = [
  { id: "terrain", name: "Terrain", type: "terrain" },
  {
    id: "fields",
    name: "Fields",
    type: "field",
    children: [
      { id: "corn", name: "Corn field", type: "field" },
      { id: "wheat", name: "Wheat field", type: "field" },
    ],
  },
  {
    id: "buildings",
    name: "Buildings",
    type: "building",
    children: [{ id: "barn", name: "Barn", type: "building" }],
  },
];

function App() {
  const [view, setView] = createSignal<"welcome" | "editor">("welcome");
  const [farmName, setFarmName] = createSignal("Untitled farm");
  const [scene, setScene] = createSignal<SceneObject[]>(DEFAULT_SCENE);

  function newFarm() {
    setFarmName("Untitled farm");
    setScene(DEFAULT_SCENE);
    setView("editor");
  }

  function openFarm(name: string) {
    setFarmName(name);
    setScene(name === "Willow Creek" ? WILLOW_CREEK_SCENE : DEFAULT_SCENE);
    setView("editor");
  }

  return (
    <Show
      when={view() === "editor"}
      fallback={<Welcome recentFarms={RECENT_FARMS} onNewFarm={newFarm} onOpenFarm={openFarm} />}
    >
      <EditorShell farmName={farmName()} scene={scene()} onClose={() => setView("welcome")} />
    </Show>
  );
}

export default App;
