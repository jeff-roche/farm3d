import { render } from "solid-js/web";
import App from "./App";
import { initTheme } from "./design-system";
import "./design-system/fonts.css";
import "./styles.css";

async function main() {
  await initTheme();

  const root = document.getElementById("root") as HTMLElement;

  if (import.meta.env.DEV && window.location.hash === "#showcase") {
    const { Showcase } = await import("./design-system/Showcase");
    render(() => <Showcase />, root);
  } else {
    render(() => <App />, root);
  }
}

main().catch((error) => {
  console.error("Failed to start the application:", error);
});
