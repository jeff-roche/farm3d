import { createSignal, For } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { useTheme } from "./design-system";
import viteLogo from "./assets/vite.svg";
import tauriLogo from "./assets/tauri.svg";
import typescriptLogo from "./assets/typescript.svg";

function App() {
  const [name, setName] = createSignal("");
  const [greetMsg, setGreetMsg] = createSignal("");
  const theme = useTheme();

  async function greet(e: SubmitEvent) {
    e.preventDefault();
    setGreetMsg(await invoke("greet", { name: name() }));
  }

  return (
    <main class="container">
      <label class="row theme-switcher">
        Theme:
        <select
          value={theme.mode()}
          onChange={(e) => theme.setThemeMode(e.currentTarget.value)}
        >
          <option value="system">System</option>
          <For each={theme.availableThemes()}>
            {(t) => <option value={t.name}>{t.name}</option>}
          </For>
        </select>
      </label>

      <h1>Welcome to Tauri</h1>

      <div class="row">
        <a href="https://vite.dev" target="_blank">
          <img src={viteLogo} class="logo vite" alt="Vite logo" />
        </a>
        <a href="https://tauri.app" target="_blank">
          <img src={tauriLogo} class="logo tauri" alt="Tauri logo" />
        </a>
        <a href="https://www.typescriptlang.org/docs" target="_blank">
          <img
            src={typescriptLogo}
            class="logo typescript"
            alt="typescript logo"
          />
        </a>
      </div>
      <p>Click on the Tauri logo to learn more about the framework</p>

      <form class="row" onSubmit={greet}>
        <input
          id="greet-input"
          placeholder="Enter a name..."
          value={name()}
          onInput={(e) => setName(e.currentTarget.value)}
        />
        <button type="submit">Greet</button>
      </form>
      <p>{greetMsg()}</p>
    </main>
  );
}

export default App;
