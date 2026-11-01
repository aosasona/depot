import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { listDomains, listProjects } from "../../lib/api/client";

/** Renders the real project-to-domain topology without inventing service links. */
export function ServiceCanvas() {
  const projects = useQuery({ queryKey: ["projects"], queryFn: listProjects });
  if (projects.isPending) return <CanvasSkeleton />;
  if (projects.isError)
    return (
      <p className="notice danger">The service topology is unavailable.</p>
    );
  if (projects.data.length === 0)
    return (
      <div className="empty-state">
        <strong>Your canvas is empty.</strong>
        <span>
          Create a project and attach a domain to see its live topology.
        </span>
        <Link className="primary-action" to="/projects">
          Create project →
        </Link>
      </div>
    );
  return (
    <section className="service-canvas" aria-label="Service topology">
      <div className="canvas-internet">
        <span>PUBLIC EDGE</span>
        <strong>Caddy</strong>
        <small>TLS + host routing</small>
      </div>
      <div className="canvas-spine" aria-hidden="true" />
      <div className="canvas-projects">
        {projects.data.map((project) => (
          <ProjectNode
            key={project.id}
            projectId={project.id}
            name={project.name}
            slug={project.slug}
          />
        ))}
      </div>
      <footer>
        <span>
          <i className="active" /> Active route
        </span>
        <span>
          <i /> Pending or unconfigured
        </span>
        <strong>{projects.data.length} services</strong>
      </footer>
    </section>
  );
}

/** Loads and displays the concrete routes attached to one project. */
function ProjectNode({
  projectId,
  name,
  slug,
}: {
  projectId: string;
  name: string;
  slug: string;
}) {
  const domains = useQuery({
    queryKey: ["projects", projectId, "domains"],
    queryFn: () => listDomains(projectId),
  });
  const live = domains.data?.some(
    (domain) => domain.routing_status === "active",
  );
  return (
    <article className={`canvas-node ${live ? "live" : ""}`}>
      <div>
        <span className="project-index">{slug.slice(0, 2).toUpperCase()}</span>
        <span>
          <strong>{name}</strong>
          <small>{slug}</small>
        </span>
        <i aria-hidden="true" />
        <span className="visually-hidden">
          {live ? "Route active" : "Route not active"}
        </span>
      </div>
      <ul>
        {domains.isPending && <li>Loading routes…</li>}
        {domains.data?.length === 0 && <li>No domain attached</li>}
        {domains.data?.map((domain) => (
          <li key={domain.id}>
            <span>{domain.hostname}</span>
            <small>{domain.routing_status}</small>
          </li>
        ))}
      </ul>
      <Link
        to="/projects/$projectId/$tab"
        params={{ projectId, tab: "overview" }}
      >
        Open service →
      </Link>
    </article>
  );
}

/** Reserves canvas space while projects load. */
function CanvasSkeleton() {
  return (
    <div className="canvas-loading" role="status">
      Mapping services…
    </div>
  );
}
