import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  ApiError,
  createDeployment,
  type Deployment,
  getJobStatus,
  listDeployments,
} from "../../lib/api/client";

/** Provides manual deployment control and live durable history for one project. */
export function DeploymentPanel({ projectId }: { projectId: string }) {
  const queryClient = useQueryClient();
  const deployments = useQuery({
    queryKey: ["projects", projectId, "deployments"],
    queryFn: () => listDeployments(projectId),
    refetchInterval: (query) =>
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
          Inspect this repository, then deploy its immutable commit.
        </p>
      )}
      <div className="deployment-list">
        {deployments.data?.map((deployment) => (
          <DeploymentRow key={deployment.id} deployment={deployment} />
        ))}
      </div>
    </div>
  );
}

/** Renders one deployment and loads its terminal Docker result when available. */
function DeploymentRow({ deployment }: { deployment: Deployment }) {
  const job = useQuery({
    queryKey: ["jobs", deployment.job_id],
    queryFn: () => getJobStatus(deployment.job_id),
    enabled: deployment.status === "succeeded",
  });
  const result = job.data?.result;
  const image = result && "image_id" in result ? result : undefined;
  return (
    <div className="deployment-row">
      <span className={`deployment-status ${deployment.status}`}>
        {deployment.status}
      </span>
      <code>{deployment.commit_sha.slice(0, 12)}</code>
      <span>{deployment.trigger_kind}</span>
      {image && <code title={image.image_id}>{image.image_tag}</code>}
      {deployment.failure_reason && (
        <p className="inspection-error">{deployment.failure_reason}</p>
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
