import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { Shell } from "./components/Shell";
import "./index.css";
import { applyFonts } from "./fonts";
import { sayMenu } from "./menu";

applyFonts();
sayMenu();

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <Shell />
  </StrictMode>,
);
