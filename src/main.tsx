import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "./App";
import { ErrorBoundary } from "./components/ErrorBoundary";
import "./styles.css";

const root = document.getElementById("root");
if (!root) throw new Error("Root interface element not available.");

createRoot(root).render(
  <StrictMode>
    <ErrorBoundary fallbackTitle="Critical application error">
      <App />
    </ErrorBoundary>
  </StrictMode>
);
