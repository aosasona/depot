import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import {
  ApiError,
  getServiceConfig,
  listDeployments,
  updateServiceConfig,
} from "../../lib/api/client";
import { ContainerFileBrowser } from "./ContainerFileBrowser";

/** Manages project-scoped persistent container directories as first-class resources. */
export function VolumesPanel({ projectId }: { projectId: string }) {
  const client = useQueryClient();
  const [path, setPath] = useState("");
  const [message, setMessage] = useState<string>();
  const [filesOpen, setFilesOpen] = useState(false);
  const config = useQuery({
    queryKey: ["projects", projectId, "config"],
    queryFn: () => getServiceConfig(projectId),
  });
  const deployments = useQuery({
    queryKey: ["projects", projectId, "deployments"],
    queryFn: () => listDeployments(projectId),
  });
  const save = useMutation({
    mutationFn: async (persistentVolumes: string[]) => {
      const current = await getServiceConfig(projectId);
      return updateServiceConfig(projectId, {
        project_kind: current.project_kind,
        builder: current.builder,
        context_path: current.context_path,
        dockerfile_path: current.dockerfile_path,
        static_output_dir: current.static_output_dir,
        exposed_port: current.exposed_port,
        healthcheck_path: current.healthcheck_path,
        persistent_volumes: persistentVolumes,
      });
    },
    onSuccess: (saved) => {
      client.setQueryData(["projects", projectId, "config"], saved);
    },
  });
  const active = deployments.data?.some(
    (deployment) =>
      deployment.active &&
      deployment.container_id &&
      deployment.runtime_desired_state !== "stopped",
  );

  /** Adds one validated absolute container directory without accepting host paths. */
  async function addVolume(event: React.FormEvent) {
    event.preventDefault();
    setMessage(undefined);
    const value = path.trim().replace(/\/$/, "");
    if (!validVolumePath(value)) {
      setMessage("Enter an absolute container directory such as /data.");
      return;
    }
    const paths = config.data?.persistent_volumes ?? [];
    if (paths.includes(value)) {
      setMessage(`${value} is already persistent.`);
      return;
    }
    try {
      await save.mutateAsync([...paths, value]);
      setPath("");
      setMessage(`${value} will be mounted on the next deployment.`);
    } catch (reason) {
      setMessage(volumeError(reason));
    }
  }

  /** Detaches one mount from future releases without deleting its Docker volume data. */
  async function removeVolume(value: string) {
    setMessage(undefined);
    try {
      const paths = (config.data?.persistent_volumes ?? []).filter(
        (candidate) => candidate !== value,
      );
      await save.mutateAsync(paths);
      setMessage(
        `${value} will be detached on the next deployment. Its stored Docker volume was not deleted.`,
      );
    } catch (reason) {
      setMessage(volumeError(reason));
    }
  }

  return (
    <div className="volumes-panel">
      <div className="surface-heading">
        <div>
          <strong>Persistent volumes</strong>
          <span>
            Keep databases, uploads, and application state across releases.
          </span>
        </div>
        <span className="volume-count">
          {config.data?.persistent_volumes.length ?? 0} / 16
        </span>
      </div>
      <div className="volume-summary">
        <span>
          <strong>Project scoped</strong>
          Each path receives a dedicated Docker named volume.
        </span>
        <span>
          <strong>Safe cutover</strong>
          Existing files are copied before the first persistent deployment.
        </span>
        <button
          className="table-action"
          type="button"
          disabled={!active}
          onClick={() => setFilesOpen(true)}
        >
          {active ? "Choose from container" : "Deploy to browse files"}
        </button>
      </div>
      <form className="volume-add-form" onSubmit={addVolume}>
        <label htmlFor="volume-path">Container directory</label>
        <span>
          <input
            id="volume-path"
            placeholder="/var/lib/app"
            spellCheck={false}
            value={path}
            onChange={(event) => setPath(event.target.value)}
          />
          <button
            className="primary-action"
            type="submit"
            disabled={save.isPending || !path.trim()}
          >
            Add volume
          </button>
        </span>
        <small>
          Use the containing directory for SQLite databases, not the database
          file itself.
        </small>
      </form>
      {config.isPending && <p className="surface-empty">Loading volumes…</p>}
      {config.isError && (
        <p className="notice danger">Persistent storage is unavailable.</p>
      )}
      {config.data?.persistent_volumes.length === 0 && (
        <div className="volume-empty">
          <strong>No persistent directories</strong>
          <span>
            Files written by the service will be lost when its container is
            replaced.
          </span>
        </div>
      )}
      <div className="volume-list">
        {config.data?.persistent_volumes.map((value) => (
          <article className="volume-row" key={value}>
            <span className="volume-icon" aria-hidden="true">
              VOL
            </span>
            <span>
              <code>{value}</code>
              <small>
                Managed Docker volume · mounted on every service release
              </small>
            </span>
            <span className="deployment-status active">Persistent</span>
            <button
              className="text-action danger-action"
              type="button"
              disabled={save.isPending}
              onClick={() => void removeVolume(value)}
            >
              Detach
            </button>
          </article>
        ))}
      </div>
      {message && <p className="config-message">{message}</p>}
      <p className="volume-retention-note">
        Detaching a path does not delete its Docker volume. This prevents
        accidental data loss but means operators must remove abandoned volumes
        manually after verifying backups.
      </p>
      {filesOpen && (
        <ContainerFileBrowser
          projectId={projectId}
          available={Boolean(active)}
          onClose={() => setFilesOpen(false)}
        />
      )}
    </div>
  );
}

/** Mirrors the server's bounded safe container destination rules. */
function validVolumePath(value: string) {
  return (
    value.startsWith("/") &&
    value !== "/" &&
    value.length <= 255 &&
    !value.includes(",") &&
    !value.split("/").some((part) => part === "." || part === "..")
  );
}

function volumeError(reason: unknown) {
  return reason instanceof ApiError || reason instanceof Error
    ? reason.message
    : "Persistent storage could not be updated.";
}
