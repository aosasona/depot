import { useForm } from "@tanstack/react-form";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { z } from "zod";
import {
  ApiError,
  getProjectGithubSource,
  getServiceConfig,
  type ProjectGithubSource,
  type ServiceConfig,
  type ServiceConfigInput,
  updateServiceConfig,
} from "../../lib/api/client";
import { RepositoryDirectoryPicker } from "./RepositoryDirectoryPicker";

const configFormSchema = z
  .object({
    project_kind: z.enum(["auto", "service", "static"]),
    builder: z.enum(["auto", "dockerfile", "railpack"]),
    context_path: z.string().trim().min(1).max(255),
    dockerfile_path: z.string().trim().min(1).max(255),
    static_output_dir: z.string().trim().max(255),
    exposed_port: z.string().trim(),
    healthcheck_path: z.string().trim().max(255),
  })
  .superRefine((value, context) => {
    if (value.project_kind === "static" && !value.static_output_dir) {
      context.addIssue({
        code: "custom",
        path: ["static_output_dir"],
        message: "Static projects require an output directory.",
      });
    }
    if (
      value.exposed_port &&
      (!/^\d+$/.test(value.exposed_port) ||
        Number(value.exposed_port) < 1 ||
        Number(value.exposed_port) > 65535)
    ) {
      context.addIssue({
        code: "custom",
        path: ["exposed_port"],
        message: "Port must be between 1 and 65535.",
      });
    }
    if (value.healthcheck_path && !value.healthcheck_path.startsWith("/")) {
      context.addIssue({
        code: "custom",
        path: ["healthcheck_path"],
        message: "Health-check paths must start with /.",
      });
    }
  });

/** Loads and reveals the editable service configuration for one project. */
export function ServiceConfigForm({
  projectId,
  suggestedPorts = [],
}: {
  projectId: string;
  suggestedPorts?: number[];
}) {
  const [expanded, setExpanded] = useState(false);
  const config = useQuery({
    queryKey: ["projects", projectId, "config"],
    queryFn: () => getServiceConfig(projectId),
    enabled: expanded,
  });
  const githubSource = useQuery({
    queryKey: ["projects", projectId, "github-source"],
    queryFn: () => getProjectGithubSource(projectId),
    enabled: expanded,
  });
  return (
    <div className="service-config">
      <button
        className="table-action config-toggle"
        type="button"
        aria-expanded={expanded}
        onClick={() => setExpanded((value) => !value)}
      >
        {expanded ? "Close configuration" : "Configure build"}
      </button>
      {expanded && (config.isPending || githubSource.isPending) && (
        <p className="config-message">Loading configuration…</p>
      )}
      {expanded && config.isError && (
        <p className="inspection-error">Configuration is unavailable.</p>
      )}
      {expanded && config.data && !githubSource.isPending && (
        <LoadedConfigForm
          key={config.data.updated_at}
          config={config.data}
          githubSource={githubSource.data ?? null}
          suggestedPorts={suggestedPorts}
        />
      )}
    </div>
  );
}

/** Edits a loaded service configuration and persists the complete replacement. */
function LoadedConfigForm({
  config,
  githubSource,
  suggestedPorts,
}: {
  config: ServiceConfig;
  githubSource: ProjectGithubSource | null;
  suggestedPorts: number[];
}) {
  const queryClient = useQueryClient();
  const [message, setMessage] = useState<string>();
  const [error, setError] = useState<string>();
  const [pathBrowser, setPathBrowser] = useState<
    "context" | "dockerfile" | "output"
  >();
  const mutation = useMutation({
    mutationFn: (input: ServiceConfigInput) =>
      updateServiceConfig(config.project_id, input),
  });
  const form = useForm({
    defaultValues: {
      project_kind: config.project_kind,
      builder: config.builder,
      context_path: config.context_path,
      dockerfile_path: config.dockerfile_path,
      static_output_dir: config.static_output_dir ?? "",
      exposed_port:
        config.exposed_port?.toString() ??
        (suggestedPorts.length === 1 ? String(suggestedPorts[0]) : ""),
      healthcheck_path: config.healthcheck_path ?? "",
    },
    onSubmit: async ({ value }) => {
      setError(undefined);
      setMessage(undefined);
      const parsed = configFormSchema.safeParse(value);
      if (!parsed.success) return setError(parsed.error.issues[0]?.message);
      try {
        const saved = await mutation.mutateAsync({
          ...parsed.data,
          static_output_dir: parsed.data.static_output_dir || null,
          exposed_port: parsed.data.exposed_port
            ? Number(parsed.data.exposed_port)
            : null,
          healthcheck_path: parsed.data.healthcheck_path || null,
        });
        queryClient.setQueryData(
          ["projects", config.project_id, "config"],
          saved,
        );
        setMessage("Configuration saved.");
      } catch (reason) {
        setError(
          reason instanceof ApiError
            ? reason.message
            : "Configuration could not be saved.",
        );
      }
    },
  });
  return (
    <form
      className="config-form"
      onSubmit={(event) => {
        event.preventDefault();
        void form.handleSubmit();
      }}
    >
      <form.Field name="project_kind">
        {(field) => (
          <label>
            Project kind
            <select
              value={field.state.value}
              onChange={(event) =>
                field.handleChange(
                  event.target.value as "auto" | "service" | "static",
                )
              }
            >
              <option value="auto">Auto-detect</option>
              <option value="service">Service</option>
              <option value="static">Static site</option>
            </select>
          </label>
        )}
      </form.Field>
      <form.Field name="builder">
        {(field) => (
          <label>
            Builder
            <select
              value={field.state.value}
              onChange={(event) =>
                field.handleChange(
                  event.target.value as "auto" | "dockerfile" | "railpack",
                )
              }
            >
              <option value="auto">Auto-detect</option>
              <option value="dockerfile">Dockerfile</option>
              <option value="railpack">Railpack</option>
            </select>
          </label>
        )}
      </form.Field>
      <form.Field name="context_path">
        {(field) => (
          <div className="config-path-field">
            <div className="config-path-control">
              <label htmlFor="build-context">Build context</label>
              <span className="config-path-input">
                <input
                  id="build-context"
                  value={field.state.value}
                  onChange={(event) => field.handleChange(event.target.value)}
                />
                <button
                  className="table-action"
                  type="button"
                  disabled={!githubSource}
                  onClick={() =>
                    setPathBrowser((value) =>
                      value === "context" ? undefined : "context",
                    )
                  }
                >
                  Browse
                </button>
              </span>
            </div>
            {pathBrowser === "context" && githubSource && (
              <RepositoryDirectoryPicker
                installationId={githubSource.installation_id}
                repositoryId={githubSource.repository_id}
                reference={githubSource.branch}
                value={field.state.value}
                title="Choose the build context"
                onChange={(path) => {
                  field.handleChange(path);
                  setPathBrowser(undefined);
                }}
              />
            )}
          </div>
        )}
      </form.Field>
      <form.Field name="dockerfile_path">
        {(field) => (
          <div className="config-path-field">
            <div className="config-path-control">
              <label htmlFor="dockerfile-path">Dockerfile</label>
              <span className="config-path-input">
                <input
                  id="dockerfile-path"
                  value={field.state.value}
                  onChange={(event) => field.handleChange(event.target.value)}
                />
                <button
                  className="table-action"
                  type="button"
                  disabled={!githubSource}
                  onClick={() =>
                    setPathBrowser((value) =>
                      value === "dockerfile" ? undefined : "dockerfile",
                    )
                  }
                >
                  Browse
                </button>
              </span>
            </div>
            {pathBrowser === "dockerfile" && githubSource && (
              <RepositoryDirectoryPicker
                installationId={githubSource.installation_id}
                repositoryId={githubSource.repository_id}
                reference={githubSource.branch}
                value={field.state.value}
                selectionMode="file"
                title="Choose the Dockerfile"
                onChange={(path) => {
                  field.handleChange(path);
                  setPathBrowser(undefined);
                }}
              />
            )}
          </div>
        )}
      </form.Field>
      <form.Field name="static_output_dir">
        {(field) => (
          <div className="config-path-field">
            <div className="config-path-control">
              <label htmlFor="static-output-path">
                Static output{" "}
                {config.builder === "dockerfile"
                  ? "(path inside final image)"
                  : "directory"}
              </label>
              <span className="config-path-input">
                <input
                  id="static-output-path"
                  placeholder={
                    config.builder === "dockerfile"
                      ? "var/www/html/site"
                      : "dist or . for plain HTML"
                  }
                  value={field.state.value}
                  onChange={(event) => field.handleChange(event.target.value)}
                />
                <button
                  className="table-action"
                  type="button"
                  disabled={!githubSource || config.builder === "dockerfile"}
                  onClick={() =>
                    setPathBrowser((value) =>
                      value === "output" ? undefined : "output",
                    )
                  }
                >
                  Browse
                </button>
              </span>
            </div>
            {pathBrowser === "output" && githubSource && (
              <form.Subscribe selector={(state) => state.values.context_path}>
                {(contextPath) => (
                  <RepositoryDirectoryPicker
                    installationId={githubSource.installation_id}
                    repositoryId={githubSource.repository_id}
                    reference={githubSource.branch}
                    value={joinRepositoryPath(contextPath, field.state.value)}
                    title="Choose committed static output"
                    onChange={(path) => {
                      field.handleChange(
                        relativeRepositoryPath(contextPath, path),
                      );
                      setPathBrowser(undefined);
                    }}
                  />
                )}
              </form.Subscribe>
            )}
          </div>
        )}
      </form.Field>
      <form.Field name="exposed_port">
        {(field) => (
          <label>
            Internal container port
            <input
              type="number"
              min="1"
              max="65535"
              placeholder="3000"
              value={field.state.value}
              onChange={(event) => field.handleChange(event.target.value)}
            />
            <small>
              The port the application listens on inside its container, not a
              public host port.
            </small>
            {suggestedPorts.length > 1 && (
              <span className="config-port-options">
                Detected in Dockerfile:{" "}
                {suggestedPorts.map((port) => (
                  <button
                    className="table-action"
                    type="button"
                    key={port}
                    onClick={() => field.handleChange(String(port))}
                  >
                    {port}
                  </button>
                ))}
              </span>
            )}
          </label>
        )}
      </form.Field>
      <form.Field name="healthcheck_path">
        {(field) => (
          <label>
            Health check
            <input
              placeholder="/health"
              value={field.state.value}
              onChange={(event) => field.handleChange(event.target.value)}
            />
          </label>
        )}
      </form.Field>
      {!githubSource && (
        <p className="config-browser-note">
          Visual repository browsing is available for projects imported through
          the GitHub App. Typed paths remain available for every Git source.
        </p>
      )}
      <button
        className="primary-action"
        type="submit"
        disabled={mutation.isPending}
      >
        {mutation.isPending ? "Saving…" : "Save configuration"}
      </button>
      {error && <p className="notice danger">{error}</p>}
      {message && <p className="notice success">{message}</p>}
    </form>
  );
}

/** Joins a build context and relative output path for repository-root browsing. */
function joinRepositoryPath(contextPath: string, outputPath: string) {
  const parts = [contextPath, outputPath]
    .filter((part) => part && part !== ".")
    .join("/");
  return parts || ".";
}

/** Converts a repository-root selection back to a path relative to build context. */
function relativeRepositoryPath(contextPath: string, selectedPath: string) {
  if (!contextPath || contextPath === ".") return selectedPath;
  if (selectedPath === contextPath) return ".";
  const prefix = `${contextPath.replace(/\/$/, "")}/`;
  return selectedPath.startsWith(prefix)
    ? selectedPath.slice(prefix.length)
    : selectedPath;
}
