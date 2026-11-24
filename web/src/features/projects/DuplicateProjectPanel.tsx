import { useForm } from "@tanstack/react-form";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { useState } from "react";
import { z } from "zod";
import { ApiError, duplicateProject, type Project } from "../../lib/api/client";

const duplicateSchema = z.object({
  name: z.string().trim().min(1).max(100),
  slug: z
    .string()
    .trim()
    .min(1)
    .max(63)
    .regex(
      /^[a-z0-9]+(?:-[a-z0-9]+)*$/,
      "Use lowercase letters, numbers, and hyphens.",
    ),
});

/** Creates a clean project identity while retaining reusable configuration. */
export function DuplicateProjectPanel({ project }: { project: Project }) {
  const client = useQueryClient();
  const navigate = useNavigate();
  const [error, setError] = useState<string>();
  const duplicate = useMutation({
    mutationFn: (input: z.infer<typeof duplicateSchema>) =>
      duplicateProject(project.id, input),
  });
  const form = useForm({
    defaultValues: {
      name: `${project.name} copy`,
      slug: `${project.slug}-copy`.slice(0, 63),
    },
    onSubmit: async ({ value }) => {
      const parsed = duplicateSchema.safeParse(value);
      if (!parsed.success) {
        setError(
          parsed.error.issues[0]?.message ?? "Check the project details.",
        );
        return;
      }
      setError(undefined);
      try {
        const created = await duplicate.mutateAsync(parsed.data);
        await client.invalidateQueries({ queryKey: ["projects"] });
        await navigate({
          to: "/projects/$projectId/$tab",
          params: { projectId: created.id, tab: "overview" },
        });
      } catch (reason) {
        setError(
          reason instanceof ApiError
            ? reason.message
            : "Depot could not duplicate this project.",
        );
      }
    },
  });

  return (
    <div className="duplicate-project-panel">
      <p>
        Copy the Git source, build settings, environment values, and persistent
        paths into a project with fresh runtime and deployment state.
      </p>
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
              New project name
              <input
                value={field.state.value}
                onChange={(event) => field.handleChange(event.target.value)}
              />
            </label>
          )}
        </form.Field>
        <form.Field name="slug">
          {(field) => (
            <label>
              New project slug
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
          disabled={duplicate.isPending}
        >
          {duplicate.isPending ? "Duplicating…" : "Duplicate project"}
        </button>
      </form>
      <small>
        Deployments, custom domains, stored files, SSH keys, and webhook
        credentials are not copied. A configured default domain is created for
        the new slug.
      </small>
      {error && (
        <p className="notice danger" role="alert">
          {error}
        </p>
      )}
    </div>
  );
}
