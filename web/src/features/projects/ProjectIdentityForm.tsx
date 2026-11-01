import { useForm } from "@tanstack/react-form";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { useState } from "react";
import { z } from "zod";
import {
  ApiError,
  deleteProject,
  type Project,
  updateProject,
} from "../../lib/api/client";

const identitySchema = z.object({
  name: z.string().trim().min(1).max(100),
  repository_url: z.url("Enter a valid repository URL."),
  branch: z.string().trim().min(1).max(255),
});

/** Edits project source metadata and contains the destructive project action. */
export function ProjectIdentityForm({ project }: { project: Project }) {
  const client = useQueryClient();
  const navigate = useNavigate();
  const [feedback, setFeedback] = useState<{
    kind: "success" | "danger";
    text: string;
  }>();
  const save = useMutation({
    mutationFn: (input: z.infer<typeof identitySchema>) =>
      updateProject(project.id, input),
  });
  const remove = useMutation({ mutationFn: () => deleteProject(project.id) });
  const form = useForm({
    defaultValues: {
      name: project.name,
      repository_url: project.repository_url,
      branch: project.branch,
    },
    onSubmit: async ({ value }) => {
      const parsed = identitySchema.safeParse(value);
      if (!parsed.success)
        return setFeedback({
          kind: "danger",
          text: parsed.error.issues[0]?.message ?? "Check the project details.",
        });
      try {
        await save.mutateAsync(parsed.data);
        await client.invalidateQueries({ queryKey: ["projects"] });
        setFeedback({ kind: "success", text: "Project details saved." });
      } catch (reason) {
        setFeedback({ kind: "danger", text: apiMessage(reason) });
      }
    },
  });

  /** Confirms and permanently removes this project. */
  async function confirmDelete() {
    if (!window.confirm(`Delete ${project.name}? This cannot be undone.`))
      return;
    try {
      await remove.mutateAsync();
      await client.invalidateQueries({ queryKey: ["projects"] });
      await navigate({ to: "/projects" });
    } catch (reason) {
      setFeedback({ kind: "danger", text: apiMessage(reason) });
    }
  }

  return (
    <div className="identity-settings">
      <form
        className="config-form"
        onSubmit={(event) => {
          event.preventDefault();
          void form.handleSubmit();
        }}
      >
        <form.Field name="name">
          {(field) => (
            <label>
              Project name
              <input
                value={field.state.value}
                onChange={(event) => field.handleChange(event.target.value)}
              />
            </label>
          )}
        </form.Field>
        <form.Field name="repository_url">
          {(field) => (
            <label>
              Repository URL
              <input
                type="url"
                value={field.state.value}
                onChange={(event) => field.handleChange(event.target.value)}
              />
            </label>
          )}
        </form.Field>
        <form.Field name="branch">
          {(field) => (
            <label>
              Branch
              <input
                value={field.state.value}
                onChange={(event) => field.handleChange(event.target.value)}
              />
            </label>
          )}
        </form.Field>
        <button
          className="primary-action"
          type="submit"
          disabled={save.isPending}
        >
          {save.isPending ? "Saving…" : "Save project"}
        </button>
      </form>
      {feedback && (
        <p className={`notice ${feedback.kind}`} role="status">
          {feedback.text}
        </p>
      )}
      <div className="danger-zone">
        <div>
          <strong>Delete project</strong>
          <span>
            Remove its configuration, deployments, domains, and history.
          </span>
        </div>
        <button
          type="button"
          className="danger-action"
          disabled={remove.isPending}
          onClick={() => void confirmDelete()}
        >
          {remove.isPending ? "Deleting…" : "Delete project"}
        </button>
      </div>
    </div>
  );
}

/** Converts API failures to safe settings feedback. */
function apiMessage(reason: unknown) {
  return reason instanceof ApiError
    ? reason.message
    : "The project could not be updated.";
}
