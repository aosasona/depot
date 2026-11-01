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
import { parseEnvironmentText } from "./environmentText";

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
  const [bulkText, setBulkText] = useState("");
  const [bulkSecrets, setBulkSecrets] = useState(true);
  const [bulkSaving, setBulkSaving] = useState(false);
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

  /** Validates and encrypts a pasted dotenv block one write-only value at a time. */
  async function saveEnvironmentBlock() {
    setError(undefined);
    try {
      const entries = parseEnvironmentText(bulkText, bulkSecrets);
      if (entries.length === 0)
        return setError("Paste at least one KEY=value entry.");
      setBulkSaving(true);
      for (const entry of entries) await upsertVariable(projectId, entry);
      setBulkText("");
      await queryClient.invalidateQueries({
        queryKey: ["projects", projectId, "variables"],
      });
    } catch (reason) {
      setError(variableError(reason));
    } finally {
      setBulkSaving(false);
    }
  }

  return (
    <div className="domain-panel environment-panel">
      <strong>Environment</strong>
      <p className="config-message">
        Values are encrypted and cannot be viewed again. Saving an existing name
        replaces it.
      </p>
      <div className="environment-bulk">
        <label>
          Paste environment block
          <textarea
            spellCheck={false}
            placeholder={"DATABASE_URL=postgres://…\nREDIS_URL=redis://…"}
            value={bulkText}
            onChange={(event) => setBulkText(event.target.value)}
          />
        </label>
        <div>
          <label className="environment-secret">
            <input
              type="checkbox"
              checked={bulkSecrets}
              onChange={(event) => setBulkSecrets(event.target.checked)}
            />
            Mark pasted values as secrets
          </label>
          <button
            className="table-action"
            type="button"
            disabled={bulkSaving}
            onClick={() => void saveEnvironmentBlock()}
          >
            {bulkSaving ? "Encrypting…" : "Save environment block"}
          </button>
        </div>
      </div>
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
  return reason instanceof ApiError || reason instanceof Error
    ? reason.message
    : "Depot could not update this runtime value.";
}
