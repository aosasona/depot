import { useQuery } from "@tanstack/react-query";
import { getJobStatus, listDeployments } from "../../lib/api/client";

/** Shows durable deployment events and bounded builder output beside deployment work. */
export function DeploymentLogsPanel({ projectId }: { projectId: string }) {
  const deployments = useQuery({
    queryKey: ["projects", projectId, "deployments"],
    queryFn: () => listDeployments(projectId),
    refetchInterval: (query) =>
      query.state.data?.some((deployment) =>
        ["queued", "building"].includes(deployment.status),
      )
        ? 1_500
        : false,
  });
  return (
    <div className="deployment-logs">
      <div className="surface-heading">
        <div>
          <strong>Deployment logs</strong>
          <span>Worker events and bounded output retained with each job.</span>
        </div>
      </div>
      {deployments.isPending && <p className="surface-empty">Loading logs…</p>}
      {deployments.isError && (
        <p className="inspection-error">Deployment logs are unavailable.</p>
      )}
      {deployments.data?.length === 0 && (
        <p className="surface-empty">No deployment jobs have run yet.</p>
      )}
      <div className="deployment-log-list">
        {deployments.data?.map((deployment, index) => (
          <DeploymentLog
            deploymentId={deployment.id}
            jobId={deployment.job_id}
            commitSha={deployment.commit_sha}
            status={deployment.status}
            defaultOpen={index === 0}
            key={deployment.id}
          />
        ))}
      </div>
    </div>
  );
}

function DeploymentLog({
  deploymentId,
  jobId,
  commitSha,
  status,
  defaultOpen,
}: {
  deploymentId: string;
  jobId: string;
  commitSha: string;
  status: string;
  defaultOpen: boolean;
}) {
  const job = useQuery({
    queryKey: ["jobs", jobId],
    queryFn: () => getJobStatus(jobId),
    refetchInterval: (query) =>
      ["queued", "running"].includes(query.state.data?.state ?? "")
        ? 1_500
        : false,
  });
  const result = job.data?.result;
  const output = result && "logs" in result ? result.logs : undefined;
  return (
    <details className="deployment-log" open={defaultOpen}>
      <summary>
        <span className={`deployment-status ${status}`}>{status}</span>
        <code>{commitSha.slice(0, 12)}</code>
        <span>{deploymentId.slice(0, 8)}</span>
        <span aria-hidden="true">⌄</span>
      </summary>
      <div className="log-stream">
        {job.isPending && <span>Loading job events…</span>}
        {job.data?.events.map((event) => (
          <div className={`log-line ${event.level}`} key={event.sequence}>
            <time>{formatTime(event.created_at)}</time>
            <span>{event.level}</span>
            <p>{event.message}</p>
          </div>
        ))}
        {output && <pre>{output}</pre>}
        {job.data?.last_error && (
          <pre className="log-error">{job.data.last_error}</pre>
        )}
        {!job.isPending && job.data?.events.length === 0 && !output && (
          <span>No output was retained for this job.</span>
        )}
      </div>
    </details>
  );
}

function formatTime(timestamp: number) {
  return new Date(timestamp * 1_000).toLocaleTimeString([], {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  });
}
