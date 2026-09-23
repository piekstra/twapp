import React from "react";
import ReactDOM from "react-dom/client";
import Hub from "./hub/Hub";

// The webview handles drag and drop itself (the window turns off Tauri's
// native handler so rows can be dragged), so a file dropped on the window
// would otherwise replace the page with that file.
for (const type of ["dragover", "drop"]) {
  document.addEventListener(type, (e) => {
    if ((e as DragEvent).dataTransfer?.types.includes("Files")) e.preventDefault();
  });
}

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <Hub />
  </React.StrictMode>
);
