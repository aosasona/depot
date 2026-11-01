const templates = [
  {
    id: "static",
    eyebrow: "STATIC",
    name: "Static site",
    description: "Build once and serve through Depot's shared static runtime.",
    stack: ["Vite / Astro", "No runtime container", "Caddy TLS"],
    action: "Start static project",
  },
  {
    id: "service",
    eyebrow: "SERVICE",
    name: "Web service",
    description:
      "A Dockerfile or Railpack application with an exposed HTTP port.",
    stack: ["Docker / Railpack", "Health checks", "Runtime logs"],
    action: "Start web service",
  },
] as const;

/** Presents supported starters and an honest blueprint for dependency-heavy apps. */
export function TemplatesPanel() {
  return (
    <div className="templates-layout">
      <section className="template-catalog" aria-label="Deployable templates">
        <header className="template-section-heading">
          <div>
            <span className="template-section-kicker">READY TO DEPLOY</span>
            <h2>Project starters</h2>
          </div>
          <p>
            Choose a deployment shape. You can inspect and change every setting
            before the first build.
          </p>
        </header>
        <div className="template-grid">
          {templates.map((template, index) => (
            <article className="template-card" key={template.id}>
              <header>
                <span className="template-number">0{index + 1}</span>
                <span className="template-kind">{template.eyebrow}</span>
                <span className="template-support">
                  <i aria-hidden="true" /> Supported
                </span>
              </header>
              <div className="template-card-copy">
                <h3>{template.name}</h3>
                <p>{template.description}</p>
              </div>
              <ul>
                {template.stack.map((item) => (
                  <li key={item}>
                    <i aria-hidden="true" />
                    {item}
                  </li>
                ))}
              </ul>
              <a
                className="template-action"
                href={`/projects?template=${template.id}`}
              >
                <span>{template.action}</span>
                <span aria-hidden="true">→</span>
              </a>
            </article>
          ))}
        </div>
      </section>
      <section className="affine-blueprint">
        <div className="blueprint-title">
          <span className="template-number">03</span>
          <div>
            <span className="template-section-kicker">
              CONNECTED APP BLUEPRINT
            </span>
            <h2>Affine</h2>
            <p>
              A real deployment plan for Affine, including the dependencies
              Depot does not provision yet.
            </p>
          </div>
          <span className="plan-badge">Operator assisted</span>
        </div>
        <section
          className="blueprint-flow"
          aria-label="Affine service dependencies"
        >
          <div className="blueprint-node primary">
            <span>DEPOT MANAGED</span>
            <strong>Affine</strong>
            <small>Web service · port 3010</small>
          </div>
          <div className="blueprint-connector" aria-hidden="true">
            <span>connects to</span>
            <i />
          </div>
          <div className="blueprint-dependencies">
            <div className="blueprint-node">
              <span>OPERATOR PROVIDED</span>
              <strong>PostgreSQL</strong>
              <small>External database</small>
            </div>
            <div className="blueprint-node">
              <span>OPERATOR PROVIDED</span>
              <strong>Redis</strong>
              <small>External cache</small>
            </div>
            <div className="blueprint-node">
              <span>OPERATOR PROVIDED</span>
              <strong>Object storage</strong>
              <small>External S3-compatible</small>
            </div>
          </div>
        </section>
        <div className="blueprint-note">
          <span>REQUIREMENTS</span>
          <div>
            <strong>Bring the data services</strong>
            <p>
              Depot deploys the Affine application container. Provide
              PostgreSQL, Redis, and object storage first, then add their
              connection values under Environment.
            </p>
          </div>
          <a className="template-action" href="/projects?template=affine">
            <span>Configure Affine</span>
            <span aria-hidden="true">→</span>
          </a>
        </div>
      </section>
    </div>
  );
}
