import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import {
  ApiError,
  createDeployment,
  type Deployment,
  getJobStatus,
  listDeployments,
  rollbackDeployment,
} from "../../lib/api/client";

/** Provides manual deployment control and live durable history for one project. */
export function DeploymentPanel({
  projectId,
  awaitingInitialDeployment = false,
}: {
  projectId: string;
  awaitingInitialDeployment?: boolean;
}) {
  const queryClient = useQueryClient();
  const deployments = useQuery({
    queryKey: ["projects", projectId, "deployments"],
    queryFn: () => listDeployments(projectId),
    refetchInterval: (query) =>
      (awaitingInitialDeployment && query.state.data?.length === 0) ||
      query.state.data?.some((item) =>
        ["queued", "building"].includes(item.status),
      )
        ? 1_500
        : false,
  });
  const create = useMutation({
    mutationFn: () => createDeployment(projectId),
    onSuccess: async () => {
      await queryClient.invalidateQueries({
        queryKey: ["projects", projectId, "deployments"],
      });
    },
  });
  return (
    <div className="deployment-panel">
      <div className="deployment-heading">
        <strong>Deployments</strong>
        <button
          className="primary-action deploy-action"
          type="button"
          disabled={
            create.isPending ||
            deployments.data?.some((item) =>
              ["queued", "building"].includes(item.status),
            )
          }
          onClick={() => create.mutate()}
        >
          {create.isPending ? "Queueing…" : "Deploy inspected commit"}
        </button>
      </div>
      {create.error && (
        <p className="inspection-error">{deploymentError(create.error)}</p>
      )}
      {deployments.isPending && (
        <p className="config-message">Loading deployment history…</p>
      )}
      {deployments.isError && (
        <p className="inspection-error">Deployment history is unavailable.</p>
      )}
      {deployments.data?.length === 0 && (
        <p className="config-message">
          {awaitingInitialDeployment
            ? "Inspecting the repository before queueing its first deployment…"
            : "Inspect this repository, then deploy its immutable commit."}
        </p>
      )}
      <div className="deployment-list">
        {deployments.data?.map((deployment) => (
          <DeploymentRow
            key={deployment.id}
            deployment={deployment}
            projectId={projectId}
          />
        ))}
      </div>
    </div>
  );
}

/** Renders one deployment and loads its terminal Docker result when available. */
function DeploymentRow({
  deployment,
  projectId,
}: {
  deployment: Deployment;
  projectId: string;
}) {
  const queryClient = useQueryClient();
  const rollback = useMutation({
    mutationFn: () => rollbackDeployment(deployment.id),
    onSuccess: () =>
      queryClient.invalidateQueries({
        queryKey: ["projects", projectId, "deployments"],
      }),
  });
  const job = useQuery({
    queryKey: ["jobs", deployment.job_id],
    queryFn: () => getJobStatus(deployment.job_id),
    enabled: deployment.status === "succeeded",
  });
  const result = job.data?.result;
  const image = result && "image_id" in result ? result : undefined;
  return (
    <div className="deployment-row">
      <Link
        className="deployment-row-link"
        to="/projects/$projectId/deployments/$deploymentId"
        params={{ projectId, deploymentId: deployment.id }}
        aria-label={`View logs for deployment ${deployment.commit_sha.slice(0, 12)}`}
      >
        <span className={`deployment-status ${deployment.status}`}>
          {deployment.active ? "active" : deployment.status}
        </span>
        <code>{deployment.commit_sha.slice(0, 12)}</code>
        <span>{deployment.trigger_kind}</span>
        {image && <code title={image.image_id}>{image.image_tag}</code>}
        {deployment.host_port && <code>127.0.0.1:{deployment.host_port}</code>}
        <span className="deployment-log-link">View logs →</span>
      </Link>
      {deployment.status === "succeeded" && !deployment.active && (
        <button
          className="text-action"
          type="button"
          disabled={rollback.isPending}
          onClick={() => rollback.mutate()}
        >
          {rollback.isPending ? "Queueing…" : "Roll back"}
        </button>
      )}
      {rollback.error && (
        <p className="inspection-error">{deploymentError(rollback.error)}</p>
      )}
      {deployment.failure_reason && (
        <p className="inspection-error">{deployment.failure_reason}</p>
      )}
      {deployment.active && deployment.runtime_route_status === "error" && (
        <p className="inspection-error">
          Route blocked:{" "}
          {deployment.runtime_route_error ?? "runtime identity did not verify"}
        </p>
      )}
    </div>
  );
}

/** Converts manual deployment failures into actionable inline feedback. */
function deploymentError(reason: unknown) {
  return reason instanceof ApiError
    ? reason.message
    : "Depot could not queue this deployment.";
}
