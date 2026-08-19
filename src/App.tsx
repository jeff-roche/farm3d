import { createSignal } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { useTheme, Panel, Button, TextField, Select } from "./design-system";
import viteLogo from "./assets/vite.svg";
import tauriLogo from "./assets/tauri.svg";
import typescriptLogo from "./assets/typescript.svg";
import styles from "./App.module.css";

function App() {
  const [name, setName] = createSignal("");
  const [greetMsg, setGreetMsg] = createSignal("");
  const theme = useTheme();

  async function greet(e: SubmitEvent) {
    e.preventDefault();
    setGreetMsg(await invoke("greet", { name: name() }));
  }

  return (
    <main class={styles.page}>
      <div class={styles.logos}>
        <a href="https://vite.dev" target="_blank">
          <img src={viteLogo} class="logo vite" alt="Vite logo" />
        </a>
        <a href="https://tauri.app" target="_blank">
          <img src={tauriLogo} class="logo tauri" alt="Tauri logo" />
        </a>
        <a href="https://www.typescriptlang.org/docs" target="_blank">
          <img src={typescriptLogo} class="logo typescript" alt="typescript logo" />
        </a>
      </div>

      <Panel title="Welcome to farm3d" class={styles.panel}>
        <p class={styles.blurb}>
          Click on the Tauri logo to learn more about the framework.
        </p>

        <form class={styles.greetForm} onSubmit={greet}>
          <TextField placeholder="Enter a name..." value={name()} onChange={setName} />
          <Button type="submit" variant="primary">
            Greet
          </Button>
        </form>
        {greetMsg() && <p>{greetMsg()}</p>}
      </Panel>

      <Panel title="Appearance" class={styles.panel}>
        <Select
          label="Theme"
          options={["system", ...theme.availableThemes().map((t) => t.name)]}
          value={theme.mode()}
          onChange={theme.setThemeMode}
        />
      </Panel>
    </main>
  );
}

export default App;
