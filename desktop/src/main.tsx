import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./index.css";
import { isTauri } from "./lib/api";
import { seedDemoRun } from "./lib/demo";

// `?demo=1` fills the store with a fake mid-run so the monitor page can be reviewed in a
// plain browser; `?demo=done` fills a finished run, which is the only state where the
// post-training inference card exists. Guarded on `!isTauri`: the real app must only ever show
// engine output.
const demo = !isTauri
  ? new URLSearchParams(window.location.search).get("demo")
  : null;
if (demo === "1" || demo === "done") {
  seedDemoRun(demo === "done");
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
