import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./styles.css";

// Linux gets its own font stacks (see `html.os-linux` in styles.css).
if (/Linux/.test(navigator.userAgent) && !/Android/.test(navigator.userAgent)) {
  document.documentElement.classList.add("os-linux");
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
