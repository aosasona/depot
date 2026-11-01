import { useForm } from "@tanstack/react-form";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { z } from "zod";
import {
  ApiError,
  createDomain,
  deleteDomain,
  listDomains,
  retryDomainReconciliation,
} from "../../lib/api/client";

const hostnameSchema = z
  .string()
  .trim()
  .toLowerCase()
  .regex(
    /^(?=.{1,253}$)(?:[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?\.)+[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/,
    "Enter a hostname such as app.example.com.",
  );

/** Manages validated public hostnames attached to one project. */
export function DomainPanel({ projectId }: { projectId: string }) {
  const queryClient = useQueryClient();
  const [error, setError] = useState<string>();
  const domains = useQuery({
    queryKey: ["projects", projectId, "domains"],
    queryFn: () => listDomains(projectId),
    refetchInterval: (query) =>
      query.state.data?.some((domain) => domain.routing_status !== "active")
        ? 2_000
        : false,
  });
  const create = useMutation({
    mutationFn: (hostname: string) => createDomain(projectId, hostname),
  });
  const remove = useMutation({
    mutationFn: deleteDomain,
    onSuccess: async () => {
      await queryClient.invalidateQueries({
        queryKey: ["projects", projectId, "domains"],
      });
    },
  });
  const retry = useMutation({
    mutationFn: retryDomainReconciliation,
    onSuccess: async () => {
      await queryClient.invalidateQueries({
        queryKey: ["projects", projectId, "domains"],
      });
    },
  });
  const hasSyncError = domains.data?.some(
    (domain) => domain.routing_status === "error",
  );
  const form = useForm({
    defaultValues: { hostname: "" },
    onSubmit: async ({ value, formApi }) => {
      setError(undefined);
      const parsed = hostnameSchema.safeParse(value.hostname);
      if (!parsed.success) return setError(parsed.error.issues[0]?.message);
      try {
        await create.mutateAsync(parsed.data);
        formApi.reset();
        await queryClient.invalidateQueries({
          queryKey: ["projects", projectId, "domains"],
        });
      } catch (reason) {
        setError(domainError(reason));
      }
    },
  });
  return (
    <div className="domain-panel">
      <div className="domain-heading">
        <strong>Domains</strong>
        {hasSyncError && (
          <button
            className="text-action"
            type="button"
            disabled={retry.isPending}
            onClick={() => retry.mutate()}
          >
            {retry.isPending ? "Retrying…" : "Retry sync"}
          </button>
        )}
      </div>
      <form
        className="domain-form"
        onSubmit={(event) => {
          event.preventDefault();
          void form.handleSubmit();
        }}
      >
        <form.Field name="hostname">
          {(field) => (
            <input
              aria-label="Project hostname"
              placeholder="app.example.com"
              value={field.state.value}
              onChange={(event) => field.handleChange(event.target.value)}
            />
          )}
        </form.Field>
        <button
          className="table-action domain-submit"
          type="submit"
          disabled={create.isPending}
        >
          Attach domain
        </button>
      </form>
      {error && <p className="inspection-error">{error}</p>}
      {remove.error && (
        <p className="inspection-error">{domainError(remove.error)}</p>
      )}
      {retry.error && (
        <p className="inspection-error">{domainError(retry.error)}</p>
      )}
      {domains.isPending && <p className="config-message">Loading domains…</p>}
      {domains.data?.length === 0 && (
        <p className="config-message">No public hostnames attached.</p>
      )}
      <div className="domain-list">
        {domains.data?.map((domain) => (
          <div className="domain-row" key={domain.id}>
            <code>{domain.hostname}</code>
            <span className={`deployment-status ${domain.routing_status}`}>
              {domain.routing_status === "error"
                ? "sync failed"
                : domain.routing_status}
            </span>
            <button
              className="text-action"
              type="button"
              disabled={remove.isPending}
              onClick={() => remove.mutate(domain.id)}
            >
              Remove
            </button>
            {domain.last_error && (
              <p className="domain-sync-error">
                The route may still be live using Caddy's last valid config.{" "}
                {domain.last_error}
              </p>
            )}
          </div>
        ))}
      </div>
    </div>
  );
}

/** Converts domain API failures into safe inline feedback. */
function domainError(reason: unknown) {
  return reason instanceof ApiError
    ? reason.message
    : "Depot could not update this domain.";
}
