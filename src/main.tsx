import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { applyAppearance, loadAppearance } from "./ui/appearance";

applyAppearance(loadAppearance(), false);

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
