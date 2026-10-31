import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import "./styles/global.css";

/** Renders the initial Depot product shell while authenticated routes are built. */
function App() {
  return (
    <main className="shell">
      <header className="masthead">
        <a className="brand" href="/" aria-label="Depot home">
          DEPOT<span aria-hidden="true">_</span>
        </a>
        <span className="technical-label">SELF-HOSTED / V0.1</span>
      </header>
      <section className="hero" aria-labelledby="hero-title">
        <p className="eyebrow">CONTROL PLANE ONLINE</p>
        <h1 id="hero-title">
          Ship your code.
          <br />
          Own the machine.
        </h1>
        <p className="lede">
          A focused deployment platform for applications and static sites on
          your own Linux VM.
        </p>
        <div className="status-panel">
          <span className="status-dot" aria-hidden="true" />
          <span>Foundation ready</span>
          <code>milestone/a</code>
        </div>
      </section>
    </main>
  );
}

const root = document.getElementById("root");
if (!root) throw new Error("Depot root element is missing");
createRoot(root).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
