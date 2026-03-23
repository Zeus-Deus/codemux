import React from "react";
import ReactDOM from "react-dom/client";
import { TooltipProvider } from "@/components/ui/tooltip";
import App from "./App";
import "./globals.css";

document.documentElement.classList.add("dark");

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <TooltipProvider>
      <App />
    </TooltipProvider>
  </React.StrictMode>,
);
