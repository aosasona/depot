import { useForm } from "@tanstack/react-form";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { z } from "zod";
import {
  ApiError,
  createProject,
  listProjects,
  type Project,
} from "../../lib/api/client";

const projectSchema = z.object({
  slug: z
    .string()
    .regex(
      /^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/,
      "Use lowercase letters, numbers, and hyphens.",
    ),
  name: z.string().trim().min(1).max(100),
  repository_url: z.url("Enter a valid HTTPS or SSH repository URL."),
  branch: z.string().trim().min(1).max(255),
});

/** Renders project creation, live project state, and intentional empty feedback. */
export function ProjectsPanel() {
  const projects = useQuery({ queryKey: ["projects"], queryFn: listProjects });
  return (
    <section className="projects-panel" aria-labelledby="projects-heading">
      <div className="section-heading">
        <div>
          <p className="eyebrow">WORKSPACE / PROJECTS</p>
          <h2 id="projects-heading">Projects</h2>
        </div>
        <span className="count-block">
          {projects.data?.length ?? 0} PROJECTS
        </span>
      </div>
      <NewProjectForm />
      {projects.isPending && <p className="panel-message">Loading projects…</p>}
      {projects.isError && (
        <p className="notice danger">Projects are unavailable.</p>
      )}
      {projects.data?.length === 0 && (
        <div className="empty-state">
          <strong>No projects yet.</strong>
          <span>
            Connect a Git repository above to establish the first deployable
            project.
          </span>
        </div>
      )}
      <div className="project-grid">
        {projects.data?.map((project) => (
          <ProjectCard key={project.id} project={project} />
        ))}
      </div>
    </section>
  );
}

/** Collects the minimal Git source details needed for a durable project record. */
function NewProjectForm() {
  const queryClient = useQueryClient();
  const [error, setError] = useState<string>();
  const mutation = useMutation({ mutationFn: createProject });
  const form = useForm({
    defaultValues: { slug: "", name: "", repository_url: "", branch: "main" },
    onSubmit: async ({ value, formApi }) => {
      setError(undefined);
      const parsed = projectSchema.safeParse(value);
      if (!parsed.success) return setError(parsed.error.issues[0]?.message);
      try {
        await mutation.mutateAsync(parsed.data);
        formApi.reset();
        await queryClient.invalidateQueries({ queryKey: ["projects"] });
      } catch (reason) {
        setError(errorMessage(reason));
      }
    },
  });
  return (
    <form
      className="project-form"
      onSubmit={(event) => {
        event.preventDefault();
        void form.handleSubmit();
      }}
    >
      <form.Field name="name">
        {(field) => (
          <input
            aria-label="Project name"
            placeholder="Project name"
            value={field.state.value}
            onChange={(event) => field.handleChange(event.target.value)}
          />
        )}
      </form.Field>
      <form.Field name="slug">
        {(field) => (
          <input
            aria-label="Project slug"
            placeholder="project-slug"
            value={field.state.value}
            onChange={(event) => field.handleChange(event.target.value)}
          />
        )}
      </form.Field>
      <form.Field name="repository_url">
        {(field) => (
          <input
            aria-label="Repository URL"
            placeholder="https://github.com/you/project.git"
            type="url"
            value={field.state.value}
            onChange={(event) => field.handleChange(event.target.value)}
          />
        )}
      </form.Field>
      <form.Field name="branch">
        {(field) => (
          <input
            aria-label="Git branch"
            placeholder="main"
            value={field.state.value}
            onChange={(event) => field.handleChange(event.target.value)}
          />
        )}
      </form.Field>
      <button
        className="primary-action"
        type="submit"
        disabled={mutation.isPending}
      >
        Create project →
      </button>
      {error && (
        <p className="notice danger" role="alert">
          {error}
        </p>
      )}
    </form>
  );
}

/** Displays repository metadata for one real project record. */
function ProjectCard({ project }: { project: Project }) {
  return (
    <article className="project-card">
      <div className="project-index">
        {project.slug.slice(0, 2).toUpperCase()}
      </div>
      <div>
        <h3>{project.name}</h3>
        <p>{project.repository_url}</p>
      </div>
      <dl>
        <div>
          <dt>Branch</dt>
          <dd>{project.branch}</dd>
        </div>
        <div>
          <dt>Status</dt>
          <dd>Not deployed</dd>
        </div>
      </dl>
    </article>
  );
}

/** Converts unknown project failures into safe inline feedback. */
function errorMessage(reason: unknown) {
  return reason instanceof ApiError
    ? reason.message
    : "Depot could not create this project.";
}
