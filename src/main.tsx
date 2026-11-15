import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { I18nProvider } from "./i18n";

// Startup timing anchor: everything before this line (HTML parse, module
// fetch) is excluded. The deltas between this and the `[startup]` marks in
// App.tsx isolate JS parse/execute + React mount from the IPC round trips.
performance.mark("boot:main-entry");

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <I18nProvider>
      <App />
    </I18nProvider>
  </React.StrictMode>,
);
