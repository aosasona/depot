import { useForm } from "@tanstack/react-form";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { z } from "zod";
import {
  ApiError,
  getServiceConfig,
  type ServiceConfig,
  type ServiceConfigInput,
  updateServiceConfig,
} from "../../lib/api/client";

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
export function ServiceConfigForm({ projectId }: { projectId: string }) {
  const [expanded, setExpanded] = useState(false);
  const config = useQuery({
    queryKey: ["projects", projectId, "config"],
    queryFn: () => getServiceConfig(projectId),
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
      {expanded && config.isPending && (
        <p className="config-message">Loading configuration…</p>
      )}
      {expanded && config.isError && (
        <p className="inspection-error">Configuration is unavailable.</p>
      )}
      {expanded && config.data && (
        <LoadedConfigForm key={config.data.updated_at} config={config.data} />
      )}
    </div>
  );
}

/** Edits a loaded service configuration and persists the complete replacement. */
function LoadedConfigForm({ config }: { config: ServiceConfig }) {
  const queryClient = useQueryClient();
  const [message, setMessage] = useState<string>();
  const [error, setError] = useState<string>();
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
      exposed_port: config.exposed_port?.toString() ?? "",
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
          <label>
            Build context
            <input
              value={field.state.value}
              onChange={(event) => field.handleChange(event.target.value)}
            />
          </label>
        )}
      </form.Field>
      <form.Field name="dockerfile_path">
        {(field) => (
          <label>
            Dockerfile
            <input
              value={field.state.value}
              onChange={(event) => field.handleChange(event.target.value)}
            />
          </label>
        )}
      </form.Field>
      <form.Field name="static_output_dir">
        {(field) => (
          <label>
            Static output{" "}
            {config.builder === "dockerfile"
              ? "(path inside final image)"
              : "directory"}
            <input
              placeholder={
                config.builder === "dockerfile" ? "var/www/html/site" : "dist"
              }
              value={field.state.value}
              onChange={(event) => field.handleChange(event.target.value)}
            />
          </label>
        )}
      </form.Field>
      <form.Field name="exposed_port">
        {(field) => (
          <label>
            Service port
            <input
              type="number"
              min="1"
              max="65535"
              placeholder="3000"
              value={field.state.value}
              onChange={(event) => field.handleChange(event.target.value)}
            />
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
