import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import {
  ApiError,
  getSystemUpdate,
  requestSystemUpdate,
} from "../../lib/api/client";

/** Gives administrators a constrained, observable path to the latest official release. */
export function SystemUpdatePanel() {
  const queryClient = useQueryClient();
  const [confirming, setConfirming] = useState(false);
  const [error, setError] = useState<string>();
  const update = useQuery({
    queryKey: ["system", "update"],
    queryFn: getSystemUpdate,
    refetchInterval: (query) =>
      ["queued", "running"].includes(query.state.data?.state ?? "")
        ? 2_000
        : 30_000,
    retry: true,
  });
  const request = useMutation({
    mutationFn: requestSystemUpdate,
    onSuccess: async () => {
      setConfirming(false);
      setError(undefined);
      await queryClient.invalidateQueries({ queryKey: ["system", "update"] });
    },
    onError: (reason) =>
      setError(
        reason instanceof ApiError
          ? reason.message
          : "Depot could not request the system update.",
      ),
  });
  const status = update.data;
  const busy = status?.state === "queued" || status?.state === "running";

  return (
    <section
      className="system-update-panel"
      aria-labelledby="system-update-heading"
    >
      <div className="password-form-heading">
        <strong id="system-update-heading">System update</strong>
        <span>
          Installs the latest verified Depot release and keeps a rollback
          backup.
        </span>
      </div>
      <div className="system-update-version">
        <span>Installed version</span>
        <strong>{status ? `v${status.version}` : "Checking…"}</strong>
      </div>
      {status && (
        <div className={`system-update-state ${status.state}`} role="status">
          <strong>{status.state.replace("_", " ")}</strong>
          <span>{status.message}</span>
          {status.updated_at && (
            <small>
              {new Date(status.updated_at * 1_000).toLocaleString()}
            </small>
          )}
        </div>
      )}
      {update.isError && !status && (
        <p className="notice danger">Depot could not load updater status.</p>
      )}
      {confirming && (
        <div className="system-update-confirm">
          <strong>Install the latest official release?</strong>
          <span>
            The dashboard will disconnect briefly while the API and worker
            restart. Running app containers are not stopped.
          </span>
          <div>
            <button
              className="primary-action"
              type="button"
              disabled={request.isPending}
              onClick={() => {
                setError(undefined);
                request.mutate();
              }}
            >
              {request.isPending ? "Queuing…" : "Install update"}
            </button>
            <button
              className="quiet-button"
              type="button"
              disabled={request.isPending}
              onClick={() => setConfirming(false)}
            >
              Cancel
            </button>
          </div>
        </div>
      )}
      {error && (
        <p className="notice danger" role="alert">
          {error}
        </p>
      )}
      {!confirming && (
        <button
          className="primary-action"
          type="button"
          disabled={!status?.available || busy || update.isPending}
          onClick={() => setConfirming(true)}
        >
          {busy ? "Update in progress…" : "Update Depot"}
        </button>
      )}
    </section>
  );
}
