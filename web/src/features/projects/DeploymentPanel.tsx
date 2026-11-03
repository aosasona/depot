import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useEffect } from "react";
import {
  ApiError,
  controlProjectRuntime,
  createDeployment,
  type Deployment,
  getJobStatus,
  getLatestRuntimeAction,
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
  const runtimeAction = useQuery({
    queryKey: ["projects", projectId, "runtime-action"],
    queryFn: () => getLatestRuntimeAction(projectId),
    refetchInterval: (query) =>
      query.state.data && ["queued", "running"].includes(query.state.data.state)
        ? 1_500
        : false,
  });
  const control = useMutation({
    mutationFn: (action: "start" | "stop" | "restart") =>
      controlProjectRuntime(projectId, action),
    onSuccess: async () => {
      await queryClient.invalidateQueries({
        queryKey: ["projects", projectId, "runtime-action"],
      });
      await queryClient.invalidateQueries({
        queryKey: ["projects", projectId, "deployments"],
      });
    },
  });
  const actionRunning =
    control.isPending ||
    (runtimeAction.data &&
      ["queued", "running"].includes(runtimeAction.data.state));
  useEffect(() => {
    if (
      runtimeAction.data &&
      !["queued", "running"].includes(runtimeAction.data.state)
    ) {
      void queryClient.invalidateQueries({
        queryKey: ["projects", projectId, "deployments"],
      });
    }
  }, [projectId, queryClient, runtimeAction.data]);
  const runtimeResult = runtimeAction.data?.result;
  const runtimeActionName =
    runtimeResult && "action" in runtimeResult
      ? runtimeResult.action
      : "operation";
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
      {runtimeAction.data && (
        <p
          className={
            runtimeAction.data.state === "failed"
              ? "inspection-error"
              : "config-message"
          }
        >
          Service {runtimeActionName} {runtimeAction.data.state}.
          {runtimeAction.data.events.at(-1)?.message &&
            ` ${runtimeAction.data.events.at(-1)?.message}`}
          {runtimeAction.data.last_error && ` ${runtimeAction.data.last_error}`}
        </p>
      )}
      {control.error && (
        <p className="inspection-error">{deploymentError(control.error)}</p>
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
            actionRunning={Boolean(actionRunning)}
            onControl={(action) => control.mutate(action)}
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
  actionRunning,
  onControl,
}: {
  deployment: Deployment;
  projectId: string;
  actionRunning: boolean;
  onControl: (action: "start" | "stop" | "restart") => void;
}) {
  const queryClient = useQueryClient();
  const rollback = useMutation({
    mutationFn: () => rollbackDeployment(deployment.id),
    onSuccess: () =>
      queryClient.invalidateQueries({
        queryKey: ["projects", projectId, "deployments"],
      }),
  });
  const confirmRollback = (label: string) => {
    if (
      window.confirm(
        `${label} this service from commit ${deployment.commit_sha.slice(0, 12)}?`,
      )
    ) {
      rollback.mutate();
    }
  };
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
        <span
          className={`deployment-status ${
            deployment.active && deployment.runtime_desired_state === "stopped"
              ? "failed"
              : deployment.status
          }`}
        >
          {deployment.active && deployment.runtime_desired_state === "stopped"
            ? "stopped"
            : deployment.active
              ? "active"
              : deployment.status}
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
          onClick={() => confirmRollback("Roll back")}
        >
          {rollback.isPending ? "Queueing…" : "Roll back"}
        </button>
      )}
      {deployment.active && deployment.container_id && (
        <div className="deployment-runtime-actions">
          {deployment.runtime_desired_state === "stopped" ? (
            <button
              className="text-action"
              type="button"
              disabled={actionRunning}
              onClick={() => onControl("start")}
            >
              Start
            </button>
          ) : (
            <>
              <button
                className="text-action"
                type="button"
                disabled={actionRunning}
                onClick={() => {
                  if (
                    window.confirm(
                      "Restart this service now? Active requests may be interrupted.",
                    )
                  )
                    onControl("restart");
                }}
              >
                Restart
              </button>
              <button
                className="text-action danger-action"
                type="button"
                disabled={actionRunning}
                onClick={() => {
                  if (
                    window.confirm(
                      "Stop this service? Its domains will return unavailable until it is started again.",
                    )
                  )
                    onControl("stop");
                }}
              >
                Stop
              </button>
              <button
                className="text-action"
                type="button"
                disabled={rollback.isPending || actionRunning}
                onClick={() => confirmRollback("Redeploy")}
              >
                {rollback.isPending ? "Queueing…" : "Redeploy"}
              </button>
            </>
          )}
        </div>
      )}
      {rollback.error && (
        <p className="inspection-error">{deploymentError(rollback.error)}</p>
      )}
      {deployment.failure_reason && (
        <p className="inspection-error">{deployment.failure_reason}</p>
      )}
      {deployment.active &&
        deployment.runtime_desired_state === "running" &&
        deployment.runtime_route_status === "error" && (
          <p className="inspection-error">
            Route blocked:{" "}
            {deployment.runtime_route_error ??
              "runtime identity did not verify"}
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
