import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { listProjects } from "../../lib/api/client";

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

const dependencies = [
  {
    id: "postgres",
    eyebrow: "DATABASE",
    name: "PostgreSQL",
    description:
      "Connect an existing PostgreSQL database through an encrypted DATABASE_URL.",
    stack: ["postgres:// DSN", "Encrypted at rest", "Canvas relationship"],
    variable: "DATABASE_URL",
  },
  {
    id: "redis",
    eyebrow: "CACHE",
    name: "Redis / Valkey",
    description:
      "Attach a compatible cache, queue, or session store without exposing its credentials.",
    stack: [
      "redis:// or rediss://",
      "Write-only secret",
      "Canvas relationship",
    ],
    variable: "REDIS_URL",
  },
  {
    id: "mysql",
    eyebrow: "DATABASE",
    name: "MySQL / MariaDB",
    description:
      "Provide a managed or self-hosted MySQL-compatible connection string to an application.",
    stack: ["mysql:// DSN", "Encrypted at rest", "Canvas relationship"],
    variable: "MYSQL_URL",
  },
  {
    id: "s3",
    eyebrow: "STORAGE",
    name: "S3-compatible storage",
    description:
      "Connect object storage such as S3, R2, MinIO, or another compatible endpoint.",
    stack: [
      "Provider connection URL",
      "Write-only secret",
      "Canvas relationship",
    ],
    variable: "S3_URL",
  },
] as const;

/** Presents supported starters and a reusable guide for dependency-heavy apps. */
export function TemplatesPanel() {
  const projects = useQuery({ queryKey: ["projects"], queryFn: listProjects });
  const [targets, setTargets] = useState<Record<string, string>>({});
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
      <section className="template-catalog" aria-label="Dependency templates">
        <header className="template-section-heading">
          <div>
            <span className="template-section-kicker">CONNECT A RESOURCE</span>
            <h2>Data and dependencies</h2>
          </div>
          <p>
            Add an existing service to a project canvas. Connection values are
            write-only, encrypted, and injected at deployment time.
          </p>
        </header>
        <div className="template-grid dependency-template-grid">
          {dependencies.map((template, index) => {
            const projectId = targets[template.id] ?? projects.data?.[0]?.id;
            return (
              <article className="template-card" key={template.id}>
                <header>
                  <span className="template-number">
                    D{String(index + 1).padStart(2, "0")}
                  </span>
                  <span className="template-kind">{template.eyebrow}</span>
                  <span className="template-support external">
                    <i aria-hidden="true" /> External
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
                <div className="dependency-template-target">
                  <label htmlFor={`dependency-project-${template.id}`}>
                    Target project
                  </label>
                  <select
                    id={`dependency-project-${template.id}`}
                    value={projectId ?? ""}
                    disabled={!projects.data?.length}
                    onChange={(event) =>
                      setTargets({
                        ...targets,
                        [template.id]: event.target.value,
                      })
                    }
                  >
                    {!projects.data?.length && (
                      <option value="">Create a project first</option>
                    )}
                    {projects.data?.map((project) => (
                      <option key={project.id} value={project.id}>
                        {project.name}
                      </option>
                    ))}
                  </select>
                </div>
                {projectId ? (
                  <a
                    className="template-action"
                    href={`/projects/${projectId}/canvas?resource=${template.id}`}
                  >
                    <span>Connect {template.variable}</span>
                    <span aria-hidden="true">→</span>
                  </a>
                ) : (
                  <a className="template-action" href="/projects">
                    <span>Create a project first</span>
                    <span aria-hidden="true">→</span>
                  </a>
                )}
              </article>
            );
          })}
        </div>
      </section>
      <section className="dependency-guide">
        <div className="blueprint-title">
          <span className="template-number">03</span>
          <div>
            <span className="template-section-kicker">DEPLOYMENT GUIDE</span>
            <h2>Apps with dependencies</h2>
            <p>
              Use the same workflow for any app that needs a database, cache,
              queue, or object storage.
            </p>
          </div>
          <span className="plan-badge">Reusable workflow</span>
        </div>
        <section
          className="blueprint-flow"
          aria-label="Application dependency workflow"
        >
          <div className="blueprint-node primary">
            <span>DEPOT MANAGED</span>
            <strong>Your application</strong>
            <small>Repository folder · HTTP service</small>
          </div>
          <div className="blueprint-connector" aria-hidden="true">
            <span>connects to</span>
            <i />
          </div>
          <div className="blueprint-dependencies">
            <div className="blueprint-node">
              <span>OPERATOR PROVIDED</span>
              <strong>Database</strong>
              <small>DATABASE_URL</small>
            </div>
            <div className="blueprint-node">
              <span>OPERATOR PROVIDED</span>
              <strong>Cache / queue</strong>
              <small>REDIS_URL</small>
            </div>
            <div className="blueprint-node">
              <span>OPERATOR PROVIDED</span>
              <strong>Object storage</strong>
              <small>S3_URL</small>
            </div>
          </div>
        </section>
        <div className="blueprint-note">
          <span>HOW IT WORKS</span>
          <div>
            <strong>
              Connections create runtime environment automatically
            </strong>
            <p>
              Create the app from its repository, provide each external
              dependency, then use Add connection on the project canvas. Depot
              stores the value encrypted under the suggested environment key and
              injects it into the next deployment.
            </p>
          </div>
          <a className="template-action" href="/projects">
            <span>Create an application</span>
            <span aria-hidden="true">→</span>
          </a>
        </div>
      </section>
    </div>
  );
}
