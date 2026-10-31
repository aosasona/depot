import { useForm } from "@tanstack/react-form";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { z } from "zod";
import {
  ApiError,
  deleteVariable,
  listVariables,
  upsertVariable,
} from "../../lib/api/client";

const variableSchema = z.object({
  name: z
    .string()
    .regex(/^[A-Z_][A-Z0-9_]{0,127}$/, "Use an uppercase environment name."),
  value: z.string().min(1, "Enter a value.").max(32_768),
  is_secret: z.boolean(),
});

/** Manages write-only encrypted runtime values for one project. */
export function EnvironmentPanel({ projectId }: { projectId: string }) {
  const queryClient = useQueryClient();
  const [error, setError] = useState<string>();
  const variables = useQuery({
    queryKey: ["projects", projectId, "variables"],
    queryFn: () => listVariables(projectId),
  });
  const save = useMutation({
    mutationFn: (input: z.infer<typeof variableSchema>) =>
      upsertVariable(projectId, input),
  });
  const remove = useMutation({
    mutationFn: (variableId: string) => deleteVariable(projectId, variableId),
    onSuccess: () =>
      queryClient.invalidateQueries({
        queryKey: ["projects", projectId, "variables"],
      }),
  });
  const form = useForm({
    defaultValues: { name: "", value: "", is_secret: true },
    onSubmit: async ({ value, formApi }) => {
      setError(undefined);
      const parsed = variableSchema.safeParse(value);
      if (!parsed.success) return setError(parsed.error.issues[0]?.message);
      try {
        await save.mutateAsync(parsed.data);
        formApi.reset();
        await queryClient.invalidateQueries({
          queryKey: ["projects", projectId, "variables"],
        });
      } catch (reason) {
        setError(variableError(reason));
      }
    },
  });

  return (
    <div className="domain-panel environment-panel">
      <strong>Environment</strong>
      <p className="config-message">
        Values are encrypted and cannot be viewed again. Saving an existing name
        replaces it.
      </p>
      <form
        className="environment-form"
        onSubmit={(event) => {
          event.preventDefault();
          void form.handleSubmit();
        }}
      >
        <form.Field name="name">
          {(field) => (
            <input
              aria-label="Environment variable name"
              placeholder="DATABASE_URL"
              autoCapitalize="characters"
              value={field.state.value}
              onChange={(event) =>
                field.handleChange(event.target.value.toUpperCase())
              }
            />
          )}
        </form.Field>
        <form.Field name="value">
          {(field) => (
            <input
              aria-label="Environment variable value"
              placeholder="Write-only value"
              type="password"
              autoComplete="new-password"
              value={field.state.value}
              onChange={(event) => field.handleChange(event.target.value)}
            />
          )}
        </form.Field>
        <form.Field name="is_secret">
          {(field) => (
            <label className="environment-secret">
              <input
                type="checkbox"
                checked={field.state.value}
                onChange={(event) => field.handleChange(event.target.checked)}
              />
              Mark as secret
            </label>
          )}
        </form.Field>
        <button
          className="table-action"
          type="submit"
          disabled={save.isPending}
        >
          {save.isPending ? "Encrypting…" : "Save value"}
        </button>
      </form>
      {error && <p className="inspection-error">{error}</p>}
      {remove.error && (
        <p className="inspection-error">{variableError(remove.error)}</p>
      )}
      {variables.data?.map((variable) => (
        <div className="domain-row" key={variable.id}>
          <code>{variable.name}</code>
          <span className="deployment-status active">
            {variable.is_secret ? "secret" : "variable"}
          </span>
          <span title="Stored value is concealed">••••••••</span>
          <button
            className="text-action"
            type="button"
            disabled={remove.isPending}
            onClick={() => remove.mutate(variable.id)}
          >
            Remove
          </button>
        </div>
      ))}
      {variables.data?.length === 0 && (
        <p className="config-message">No runtime values configured.</p>
      )}
    </div>
  );
}

/** Converts variable API failures into safe inline feedback. */
function variableError(reason: unknown) {
  return reason instanceof ApiError
    ? reason.message
    : "Depot could not update this runtime value.";
}
