import { render } from "solid-js/web";
import App from "./App";
import { initTheme } from "./design-system";
import "./styles.css";

initTheme();

render(() => <App />, document.getElementById("root") as HTMLElement);
