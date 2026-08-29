import React from "react";
import ReactDOM from "react-dom/client";
import "@fontsource/ibm-plex-sans/400.css";
import "@fontsource/ibm-plex-sans/500.css";
import "@fontsource/ibm-plex-sans/600.css";
import "@fontsource/ibm-plex-mono/400.css";
import "@fontsource/ibm-plex-mono/600.css";
import "@fontsource/spectral/600.css";
import App from "./App";
import "./App.css";
import { hasPreviewParam } from "./dev/preview";
import { SkinsGallery } from "./dev/SkinsGallery";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    {hasPreviewParam("skins-preview") ? <SkinsGallery /> : <App />}
  </React.StrictMode>,
);
