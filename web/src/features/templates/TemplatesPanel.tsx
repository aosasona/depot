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
      <section className="template-grid" aria-label="Deployable templates">
        {templates.map((template, index) => (
          <article className="template-card" key={template.id}>
            <span className="template-number">0{index + 1}</span>
            <span className="eyebrow">{template.eyebrow}</span>
            <h2>{template.name}</h2>
            <p>{template.description}</p>
            <ul>
              {template.stack.map((item) => (
                <li key={item}>{item}</li>
              ))}
            </ul>
            <a
              className="primary-action"
              href={`/projects?template=${template.id}`}
            >
              {template.action} →
            </a>
          </article>
        ))}
      </section>
      <section className="affine-blueprint">
        <div className="blueprint-title">
          <span className="template-number">03</span>
          <div>
            <span className="eyebrow">CONNECTED APP BLUEPRINT</span>
            <h2>Affine</h2>
            <p>
              A real deployment plan for Affine, including the dependencies
              Depot does not provision yet.
            </p>
          </div>
          <span className="plan-badge">OPERATOR ASSISTED</span>
        </div>
        <section
          className="blueprint-flow"
          aria-label="Affine service dependencies"
        >
          <div className="blueprint-node primary">
            <strong>Affine</strong>
            <small>Web service · port 3010</small>
          </div>
          <span>→</span>
          <div className="blueprint-dependencies">
            <div className="blueprint-node">
              <strong>PostgreSQL</strong>
              <small>External database</small>
            </div>
            <div className="blueprint-node">
              <strong>Redis</strong>
              <small>External cache</small>
            </div>
            <div className="blueprint-node">
              <strong>Object storage</strong>
              <small>External S3-compatible</small>
            </div>
          </div>
        </section>
        <div className="blueprint-note">
          <strong>Why this is not one click yet</strong>
          <p>
            Depot safely deploys the Affine application container, but v0.1
            deliberately does not create or own databases, Redis, or object
            storage. Provide those services first, then add their values under
            Environment.
          </p>
          <a href="/projects?template=affine">Configure the Affine service →</a>
        </div>
      </section>
    </div>
  );
}
