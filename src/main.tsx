import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import ErrorBoundary from "./components/ErrorBoundary";
import { initGlobalClipboard } from "./clipboard";
import "@xterm/xterm/css/xterm.css";
import "./styles.css";

// copy/paste in inputs & selections must work even when WebView2 blocks the webview's
// own clipboard access — install the native-first interception before anything renders
initGlobalClipboard();

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <ErrorBoundary>
      <App />
    </ErrorBoundary>
  </React.StrictMode>,
);
