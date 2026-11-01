import { useMutation, useQuery } from "@tanstack/react-query";
import { Link, useNavigate } from "@tanstack/react-router";
import { useState } from "react";
import {
  ApiError,
  getJobStatus,
  inspectProject,
  listProjects,
  type Project,
} from "../../lib/api/client";
import { DeploymentLogsPanel } from "./DeploymentLogsPanel";
import { DeploymentPanel } from "./DeploymentPanel";
import { DomainPanel } from "./DomainPanel";
import { EnvironmentPanel } from "./EnvironmentPanel";
import { GithubImportPanel } from "./GithubImportPanel";
import { ProjectCanvas } from "./ProjectCanvas";
import { ProjectIdentityForm } from "./ProjectIdentityForm";
import { ProjectOverview } from "./ProjectOverview";
import { ServiceConfigForm } from "./ServiceConfigForm";
import { SshCredentialPanel } from "./SshCredentialPanel";
import { WebhookPanel } from "./WebhookPanel";

/** Renders project creation, live project state, and intentional empty feedback. */
export function ProjectsPanel({
  projectId,
  tab = "overview",
}: {
  projectId?: string;
  tab?: ProjectTab;
}) {
  const navigate = useNavigate();
  const projects = useQuery({ queryKey: ["projects"], queryFn: listProjects });
  const selectedProject = projects.data?.find(
    (project) => project.id === projectId,
  );
  if (selectedProject) {
    return <ProjectWorkspace project={selectedProject} tab={tab} />;
  }
  if (projectId && projects.isSuccess) {
    return (
      <section className="project-not-found">
        <span>404</span>
        <h2>Project not found</h2>
        <p>This project may have been removed or the URL is incorrect.</p>
        <Link to="/projects">Return to projects</Link>
      </section>
    );
  }
  return (
    <section className="projects-panel" aria-labelledby="projects-heading">
      <h2 className="visually-hidden" id="projects-heading">
        Projects
      </h2>
      <div className="project-list-toolbar">
        <span>Repository-backed services</span>
        <strong>{String(projects.data?.length ?? 0).padStart(2, "0")}</strong>
      </div>
      <GithubImportPanel />
      {projects.isPending && <p className="panel-message">Loading projects…</p>}
      {projects.isError && (
        <p className="notice danger">Projects are unavailable.</p>
      )}
      {projects.data?.length === 0 && (
        <div className="empty-state">
          <strong>No projects yet.</strong>
          <span>
            Connect a Git repository above to establish the first deployable
            project.
          </span>
        </div>
      )}
      <div className="project-grid">
        {projects.data?.map((project) => (
          <ProjectListCard
            key={project.id}
            project={project}
            onSelect={() =>
              void navigate({
                to: "/projects/$projectId/$tab",
                params: { projectId: project.id, tab: "overview" },
              })
            }
          />
        ))}
      </div>
    </section>
  );
}

/** Presents one project as a focused selection instead of an expanded control wall. */
function ProjectListCard({
  project,
  onSelect,
}: {
  project: Project;
  onSelect: () => void;
}) {
  return (
    <button className="project-list-card" type="button" onClick={onSelect}>
      <span className="project-index">
        {project.slug.slice(0, 2).toUpperCase()}
      </span>
      <span className="project-list-identity">
        <strong>{project.name}</strong>
        <small>{project.repository_url}</small>
      </span>
      <span className="project-branch">{project.branch}</span>
      <span
        className={`project-readiness ${project.latest_inspection_job_id ? "ready" : "waiting"}`}
      >
        <i aria-hidden="true" />
        {project.latest_inspection_job_id ? "Inspected" : "Setup required"}
      </span>
      <span className="project-open" aria-hidden="true">
        →
      </span>
    </button>
  );
}

export type ProjectTab =
  | "overview"
  | "canvas"
  | "deployments"
  | "logs"
  | "environment"
  | "domains"
  | "settings";

const projectTabs: Array<{ id: ProjectTab; label: string }> = [
  { id: "overview", label: "Overview" },
  { id: "canvas", label: "Canvas" },
  { id: "deployments", label: "Deployments" },
  { id: "logs", label: "Logs" },
  { id: "environment", label: "Environment" },
  { id: "domains", label: "Domains" },
  { id: "settings", label: "Settings" },
];

/** Hosts project operations in stable, task-oriented views. */
function ProjectWorkspace({
  project,
  tab,
}: {
  project: Project;
  tab: ProjectTab;
}) {
  const [jobId, setJobId] = useState<string | undefined>(
    project.latest_inspection_job_id ?? undefined,
  );
  const inspection = useMutation({
    mutationFn: () => inspectProject(project.id),
    onSuccess: (result) => setJobId(result.job_id),
  });
  const job = useQuery({
    queryKey: ["jobs", jobId],
    queryFn: () => getJobStatus(jobId ?? ""),
    enabled: Boolean(jobId),
    refetchInterval: (query) => {
      const state = query.state.data?.state;
      return state === "queued" || state === "running" ? 1_500 : false;
    },
  });
  const inspectionResult =
    job.data?.result && "builder" in job.data.result
      ? job.data.result
      : undefined;
  return (
    <section className="project-workspace" aria-labelledby="project-title">
      <Link className="project-back" to="/projects">
        ← All projects
      </Link>
      <div className="project-workspace-heading">
        <div className="project-index">
          {project.slug.slice(0, 2).toUpperCase()}
        </div>
        <div>
          <span className="project-heading-kicker">
            PROJECT / {project.slug}
          </span>
          <h2 id="project-title">{project.name}</h2>
          <p>{project.repository_url}</p>
        </div>
        <span className="project-branch">{project.branch}</span>
      </div>
      <nav className="project-tabs" aria-label={`${project.name} sections`}>
        {projectTabs.map((item) => (
          <Link
            className={tab === item.id ? "active" : undefined}
            key={item.id}
            to="/projects/$projectId/$tab"
            params={{ projectId: project.id, tab: item.id }}
          >
            {item.label}
          </Link>
        ))}
      </nav>
      <div className="project-tab-content">
        {tab === "overview" && (
          <div className="overview-stack">
            <ProjectOverview project={project} />
            <section className="project-surface inspection-surface">
              <div className="surface-heading">
                <div>
                  <strong>Source inspection</strong>
                  <span>Detect the build plan at the current branch head.</span>
                </div>
                <button
                  className="table-action inspection-action"
                  type="button"
                  disabled={
                    inspection.isPending ||
                    job.data?.state === "queued" ||
                    job.data?.state === "running"
                  }
                  onClick={() => inspection.mutate()}
                >
                  {job.data?.state === "queued"
                    ? "Inspection queued"
                    : job.data?.state === "running"
                      ? "Inspecting…"
                      : "Inspect source"}
                </button>
              </div>
              {inspectionResult ? (
                <div className="inspection-result">
                  <strong>{inspectionResult.builder}</strong>
                  <code>{inspectionResult.commit_sha.slice(0, 12)}</code>
                  <span>{inspectionResult.reason}</span>
                  {inspectionResult.has_mise_config && (
                    <span>mise configuration detected.</span>
                  )}
                </div>
              ) : (
                <p className="surface-empty">
                  No successful inspection is loaded for this project yet.
                </p>
              )}
              {job.data?.state === "failed" && (
                <p className="inspection-error">
                  {job.data.last_error ?? "Inspection failed."}
                </p>
              )}
              {inspection.error && (
                <p className="inspection-error">
                  {errorMessage(inspection.error)}
                </p>
              )}
            </section>
          </div>
        )}
        {tab === "deployments" && (
          <section className="project-surface">
            <DeploymentPanel projectId={project.id} />
          </section>
        )}
        {tab === "canvas" && <ProjectCanvas project={project} />}
        {tab === "logs" && (
          <section className="project-surface">
            <DeploymentLogsPanel projectId={project.id} />
          </section>
        )}
        {tab === "environment" && (
          <section className="project-surface">
            <EnvironmentPanel projectId={project.id} />
          </section>
        )}
        {tab === "domains" && (
          <section className="project-surface">
            <DomainPanel projectId={project.id} />
          </section>
        )}
        {tab === "settings" && (
          <div className="project-settings-grid">
            <section className="project-surface">
              <div className="surface-heading">
                <div>
                  <strong>Project details</strong>
                  <span>Name, source repository, and tracked branch.</span>
                </div>
              </div>
              <ProjectIdentityForm project={project} />
            </section>
            <section className="project-surface">
              <div className="surface-heading">
                <div>
                  <strong>Build and runtime</strong>
                  <span>Builder, paths, port, and health checks.</span>
                </div>
              </div>
              <ServiceConfigForm projectId={project.id} />
            </section>
            {project.repository_url.startsWith("ssh://") && (
              <section className="project-surface">
                <SshCredentialPanel projectId={project.id} />
              </section>
            )}
            <section className="project-surface">
              <WebhookPanel projectId={project.id} />
            </section>
          </div>
        )}
      </div>
    </section>
  );
}

/** Converts unknown project failures into safe inline feedback. */
function errorMessage(reason: unknown) {
  return reason instanceof ApiError
    ? reason.message
    : "Depot could not create this project.";
}
