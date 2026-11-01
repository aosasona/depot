import { type UseQueryResult, useQuery } from "@tanstack/react-query";
import { useState } from "react";
import {
  type Deployment,
  getJobStatus,
  listDeployments,
  requestRuntimeLogs,
} from "../../lib/api/client";

/** Shows durable deployment events and bounded builder output beside deployment work. */
export function DeploymentLogsPanel({ projectId }: { projectId: string }) {
  const [view, setView] = useState<"runtime" | "build">("runtime");
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
          <strong>Logs</strong>
          <span>Live service output and durable build history.</span>
        </div>
        <fieldset className="log-view-tabs">
          <legend>Log type</legend>
          <button
            className={view === "runtime" ? "active" : undefined}
            type="button"
            onClick={() => setView("runtime")}
          >
            Runtime
          </button>
          <button
            className={view === "build" ? "active" : undefined}
            type="button"
            onClick={() => setView("build")}
          >
            Build
          </button>
        </fieldset>
      </div>
      {view === "runtime" && <RuntimeLogs projectId={projectId} />}
      {view === "build" && <BuildLogs deployments={deployments} />}
    </div>
  );
}

/** Polls recent output for the active service while the runtime view is open. */
function RuntimeLogs({ projectId }: { projectId: string }) {
  const request = useQuery({
    queryKey: ["projects", projectId, "runtime-log-request"],
    queryFn: () => requestRuntimeLogs(projectId),
    refetchInterval: 5_000,
  });
  const job = useQuery({
    queryKey: ["jobs", request.data?.job_id],
    queryFn: () => getJobStatus(request.data?.job_id ?? ""),
    enabled: Boolean(request.data?.job_id),
    refetchInterval: (query) =>
      ["queued", "running"].includes(query.state.data?.state ?? "")
        ? 750
        : false,
  });
  const runtime =
    job.data?.result && "status" in job.data.result ? job.data.result : null;
  if (request.isPending || job.isPending)
    return <p className="surface-empty">Connecting to the active service…</p>;
  if (request.isError || job.isError || job.data?.state === "failed")
    return (
      <p className="inspection-error">
        Runtime logs are unavailable. Check the Depot worker service.
      </p>
    );
  if (!runtime)
    return <p className="surface-empty">Collecting runtime output…</p>;
  if (runtime.status === "static")
    return (
      <p className="surface-empty">
        Static sites have no running process. Build output is available under
        Build.
      </p>
    );
  if (runtime.status === "not_deployed")
    return (
      <p className="surface-empty">
        Deploy this project to start collecting runtime output.
      </p>
    );
  return (
    <div className="runtime-console">
      <div>
        <span>ACTIVE RELEASE</span>
        <code>{runtime.deployment_id?.slice(0, 12)}</code>
        <time>
          Updated {new Date(runtime.fetched_at * 1_000).toLocaleString()}
        </time>
      </div>
      <pre>{runtime.logs || "The service has not written any output yet."}</pre>
    </div>
  );
}

/** Renders retained worker events and build output for all deployment attempts. */
function BuildLogs({
  deployments,
}: {
  deployments: UseQueryResult<Deployment[], Error>;
}) {
  return (
    <>
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
    </>
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
  return new Date(timestamp * 1_000).toLocaleString([], {
    year: "numeric",
    month: "short",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  });
}
