import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { Shell } from "./components/Shell";
import "./index.css";
import { applyFonts } from "./fonts";

applyFonts();

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <Shell />
  </StrictMode>,
);
