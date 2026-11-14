import { useForm } from "@tanstack/react-form";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { z } from "zod";
import {
  ApiError,
  deleteVariable,
  listVariables,
  revealVariable,
  upsertVariable,
} from "../../lib/api/client";
import {
  parseEnvironmentText,
  serializeEnvironmentText,
} from "./environmentText";

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
  const [entryMode, setEntryMode] = useState<"single" | "bulk">("single");
  const [bulkText, setBulkText] = useState("");
  const [bulkSecrets, setBulkSecrets] = useState(true);
  const [bulkSaving, setBulkSaving] = useState(false);
  const [bulkLoading, setBulkLoading] = useState(false);
  const [showEntryValue, setShowEntryValue] = useState(false);
  const [revealedValues, setRevealedValues] = useState<Record<string, string>>(
    {},
  );
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
  const reveal = useMutation({
    mutationFn: (variableId: string) => revealVariable(projectId, variableId),
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

  /** Loads the complete saved environment into the explicitly selected raw editor. */
  async function openRawEditor() {
    setError(undefined);
    setEntryMode("bulk");
    setBulkLoading(true);
    try {
      const current = variables.data ?? (await variables.refetch()).data ?? [];
      const values = await Promise.all(
        current.map(async (variable) => ({
          name: variable.name,
          value: (await revealVariable(projectId, variable.id)).value,
        })),
      );
      setBulkText(serializeEnvironmentText(values));
    } catch (reason) {
      setError(variableError(reason));
      setEntryMode("single");
    } finally {
      setBulkLoading(false);
    }
  }

  /** Reconciles the raw editor with encrypted storage one bounded value at a time. */
  async function saveEnvironmentBlock() {
    setError(undefined);
    try {
      const entries = parseEnvironmentText(bulkText, bulkSecrets);
      setBulkSaving(true);
      const existing = variables.data ?? [];
      for (const entry of entries) {
        const current = existing.find(
          (variable) => variable.name === entry.name,
        );
        await upsertVariable(projectId, {
          ...entry,
          is_secret: current?.is_secret ?? bulkSecrets,
        });
      }
      const retained = new Set(entries.map((entry) => entry.name));
      for (const variable of existing)
        if (!retained.has(variable.name))
          await deleteVariable(projectId, variable.id);
      await queryClient.invalidateQueries({
        queryKey: ["projects", projectId, "variables"],
      });
    } catch (reason) {
      setError(variableError(reason));
    } finally {
      setBulkSaving(false);
    }
  }

  /** Keeps decrypted values only in this mounted panel and forgets them when hidden. */
  async function toggleRevealedValue(variableId: string) {
    setError(undefined);
    if (revealedValues[variableId] !== undefined) {
      setRevealedValues((current) => {
        const next = { ...current };
        delete next[variableId];
        return next;
      });
      return;
    }
    try {
      const response = await reveal.mutateAsync(variableId);
      setRevealedValues((current) => ({
        ...current,
        [variableId]: response.value,
      }));
    } catch (reason) {
      setError(variableError(reason));
    }
  }

  return (
    <div className="domain-panel environment-panel">
      <strong>Environment</strong>
      <p className="config-message">
        Values are encrypted at rest. Revealing saved values is explicit and
        audited. Saving an existing name replaces it.
      </p>
      <fieldset
        className="environment-entry-switch"
        aria-label="Variable entry mode"
      >
        <button
          type="button"
          aria-pressed={entryMode === "single"}
          onClick={() => {
            setBulkText("");
            setEntryMode("single");
          }}
        >
          Key-value pair
        </button>
        <button
          type="button"
          aria-pressed={entryMode === "bulk"}
          disabled={bulkLoading}
          onClick={() => void openRawEditor()}
        >
          {bulkLoading ? "Loading…" : "Raw text"}
        </button>
      </fieldset>
      {entryMode === "bulk" ? (
        <div className="environment-bulk">
          <label>
            Project environment
            <textarea
              autoCapitalize="none"
              autoCorrect="off"
              spellCheck={false}
              placeholder={"DATABASE_URL=postgres://…\nREDIS_URL=redis://…"}
              value={bulkText}
              onChange={(event) => setBulkText(event.target.value)}
            />
          </label>
          <p className="environment-raw-note">
            This document is the complete project environment. Removing a line
            removes that saved variable when you save.
          </p>
          <div>
            <label className="environment-secret">
              <input
                type="checkbox"
                checked={bulkSecrets}
                onChange={(event) => setBulkSecrets(event.target.checked)}
              />
              Mark new values as secrets
            </label>
            <button
              className="table-action"
              type="button"
              disabled={bulkSaving}
              onClick={() => void saveEnvironmentBlock()}
            >
              {bulkSaving ? "Saving…" : "Save environment"}
            </button>
          </div>
        </div>
      ) : (
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
                className="environment-key-input"
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
              <div className="environment-value-input">
                <input
                  aria-label="Environment variable value"
                  placeholder="Write-only value"
                  type={showEntryValue ? "text" : "password"}
                  autoComplete="new-password"
                  autoCapitalize="none"
                  autoCorrect="off"
                  spellCheck={false}
                  value={field.state.value}
                  onChange={(event) => field.handleChange(event.target.value)}
                />
                <button
                  type="button"
                  aria-pressed={showEntryValue}
                  onClick={() => setShowEntryValue((current) => !current)}
                >
                  {showEntryValue ? "Hide" : "Show"}
                </button>
              </div>
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
      )}
      {error && <p className="inspection-error">{error}</p>}
      {remove.error && (
        <p className="inspection-error">{variableError(remove.error)}</p>
      )}
      {entryMode === "single" &&
        variables.data?.map((variable) => (
          <div className="domain-row" key={variable.id}>
            <code>{variable.name}</code>
            <span
              className={`deployment-status ${
                variable.is_secret
                  ? "environment-label-secret"
                  : "environment-label-variable"
              }`}
            >
              {variable.is_secret ? "secret" : "variable"}
            </span>
            <span className="environment-stored-value">
              <span title="Stored encrypted value">
                {revealedValues[variable.id] ?? "••••••••"}
              </span>
              <button
                className="text-action"
                type="button"
                aria-pressed={revealedValues[variable.id] !== undefined}
                disabled={reveal.isPending && reveal.variables === variable.id}
                onClick={() => void toggleRevealedValue(variable.id)}
              >
                {reveal.isPending && reveal.variables === variable.id
                  ? "Loading…"
                  : revealedValues[variable.id] !== undefined
                    ? "Hide"
                    : "Show"}
              </button>
            </span>
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
      {entryMode === "single" && variables.data?.length === 0 && (
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
