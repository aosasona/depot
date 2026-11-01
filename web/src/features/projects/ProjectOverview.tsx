import { useQuery } from "@tanstack/react-query";
import {
  getServiceConfig,
  listDeployments,
  listDomains,
  type Project,
} from "../../lib/api/client";

/** Summarizes live project state from the existing focused APIs. */
export function ProjectOverview({ project }: { project: Project }) {
  const config = useQuery({
    queryKey: ["projects", project.id, "config"],
    queryFn: () => getServiceConfig(project.id),
  });
  const deployments = useQuery({
    queryKey: ["projects", project.id, "deployments"],
    queryFn: () => listDeployments(project.id),
  });
  const domains = useQuery({
    queryKey: ["projects", project.id, "domains"],
    queryFn: () => listDomains(project.id),
  });
  const latest = deployments.data?.[0];
  const active = deployments.data?.find((deployment) => deployment.active);
  const primaryDomain = domains.data?.find(
    (domain) => domain.routing_status === "active",
  );
  const loading =
    config.isPending || deployments.isPending || domains.isPending;

  if (loading) {
    return <OverviewSkeleton />;
  }
  if (config.isError || deployments.isError || domains.isError) {
    return (
      <p className="inspection-error">
        Some project status could not be loaded.
      </p>
    );
  }

  return (
    <div className="overview-stack">
      <section className="overview-metrics" aria-label="Project status">
        <Metric
          label="Status"
          value={active ? "Live" : (latest?.status ?? "Not deployed")}
          tone={active ? "success" : undefined}
        />
        <Metric
          label="Deployments"
          value={String(deployments.data?.length ?? 0)}
        />
        <Metric label="Domains" value={String(domains.data?.length ?? 0)} />
        <Metric label="Runtime" value={config.data?.project_kind ?? "auto"} />
      </section>
      <div className="project-overview-grid">
        <section className="project-surface project-source-card">
          <div className="surface-heading">
            <div>
              <strong>Latest release</strong>
              <span>Current deployment and routing state.</span>
            </div>
          </div>
          <dl>
            <div>
              <dt>Commit</dt>
              <dd>
                <code>
                  {(active ?? latest)?.commit_sha.slice(0, 12) ?? "—"}
                </code>
              </dd>
            </div>
            <div>
              <dt>State</dt>
              <dd>{active ? "Active" : (latest?.status ?? "No release")}</dd>
            </div>
            <div>
              <dt>Primary domain</dt>
              <dd>
                {primaryDomain ? (
                  <a
                    href={`https://${primaryDomain.hostname}`}
                    target="_blank"
                    rel="noreferrer"
                  >
                    {primaryDomain.hostname} ↗
                  </a>
                ) : (
                  "Not connected"
                )}
              </dd>
            </div>
            <div>
              <dt>Last activity</dt>
              <dd>
                {latest ? formatDate(latest.updated_at) : "No deployments yet"}
              </dd>
            </div>
          </dl>
        </section>
        <section className="project-surface project-source-card">
          <div className="surface-heading">
            <div>
              <strong>Build plan</strong>
              <span>Configuration used by the next deployment.</span>
            </div>
          </div>
          <dl>
            <div>
              <dt>Kind</dt>
              <dd>{config.data?.project_kind}</dd>
            </div>
            <div>
              <dt>Builder</dt>
              <dd>{config.data?.builder}</dd>
            </div>
            <div>
              <dt>Context</dt>
              <dd>
                <code>{config.data?.context_path}</code>
              </dd>
            </div>
            <div>
              <dt>
                {config.data?.project_kind === "static" ? "Output" : "Port"}
              </dt>
              <dd>
                {config.data?.project_kind === "static"
                  ? (config.data.static_output_dir ?? "Detect")
                  : (config.data?.exposed_port ?? "Not set")}
              </dd>
            </div>
          </dl>
        </section>
      </div>
    </div>
  );
}

/** Renders one compact project metric. */
function Metric({
  label,
  value,
  tone,
}: {
  label: string;
  value: string;
  tone?: string;
}) {
  return (
    <div className={`overview-metric ${tone ?? ""}`}>
      <span>{label}</span>
      <strong>{value}</strong>
    </div>
  );
}

/** Keeps the overview geometry stable while its parallel requests resolve. */
function OverviewSkeleton() {
  return (
    <div
      className="overview-metrics skeleton-row"
      role="status"
      aria-label="Loading project status"
    >
      <i />
      <i />
      <i />
      <i />
    </div>
  );
}

/** Formats a Unix timestamp for concise operational display. */
function formatDate(timestamp: number) {
  return new Intl.DateTimeFormat(undefined, {
    dateStyle: "medium",
    timeStyle: "short",
  }).format(timestamp * 1_000);
}
