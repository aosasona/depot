import { useForm } from "@tanstack/react-form";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { type ReactNode, useState } from "react";
import { z } from "zod";
import {
  ApiError,
  bootstrapAdmin,
  getSystemReadiness,
  login,
  logout,
  type User,
} from "../../lib/api/client";
import { ProjectsPanel, type ProjectTab } from "../projects/ProjectsPanel";
import { TemplatesPanel } from "../templates/TemplatesPanel";
import { UsersPanel } from "../users/UsersPanel";
import { bootstrapStatusQuery, currentUserQuery } from "./queries";

const setupSchema = z.object({
  token: z
    .string()
    .length(64, "Paste the 64-character token from depot setup."),
  email: z.email("Enter a valid email address."),
  password: z.string().min(12, "Use at least 12 characters."),
});
const loginSchema = z.object({
  email: z.email("Enter a valid email address."),
  password: z.string().min(1, "Enter your password."),
});

export type DashboardLocation =
  | { kind: "projects" }
  | {
      kind: "project";
      projectId: string;
      tab: ProjectTab;
      deploymentId?: string;
    }
  | { kind: "templates" }
  | { kind: "team" };

/** Selects setup, login, or the requested dashboard location from real API state. */
export function AuthEntryPage({
  dashboard = { kind: "projects" },
}: {
  dashboard?: DashboardLocation;
}) {
  const setup = useQuery(bootstrapStatusQuery());
  const session = useQuery({
    ...currentUserQuery(),
    enabled: setup.data?.initialized === true,
  });

  if (setup.isPending) return <AuthFrame label="Starting control plane…" />;
  if (setup.isError)
    return <AuthFrame label="Depot could not read instance state." danger />;
  if (!setup.data.initialized) return <SetupForm />;
  if (session.isPending) return <AuthFrame label="Checking session…" />;
  if (session.data)
    return <Dashboard user={session.data} location={dashboard} />;
  return <LoginForm />;
}

/** Frames authentication screens in Depot's high-contrast visual language. */
function AuthFrame({
  children,
  label,
  danger = false,
}: {
  children?: ReactNode;
  label?: string;
  danger?: boolean;
}) {
  return (
    <main className="auth-shell">
      <header className="masthead">
        <a className="brand" href="/" aria-label="Depot home">
          DEPOT<span aria-hidden="true">_</span>
        </a>
        <span className="technical-label">SELF-HOSTED / V0.1</span>
      </header>
      <section className="auth-grid">
        <div className="auth-intro">
          <p className="eyebrow">ONE MACHINE / YOUR RULES</p>
          <h1>
            Ship code.
            <br />
            Own the machine.
          </h1>
          <p className="lede">
            Build, deploy, route, and inspect personal projects from one focused
            control plane.
          </p>
        </div>
        <div className="auth-panel">
          {label ? (
            <p className={danger ? "notice danger" : "notice"}>{label}</p>
          ) : (
            children
          )}
        </div>
      </section>
    </main>
  );
}

/** Collects the one-time operator token and creates the first administrator. */
function SetupForm() {
  const queryClient = useQueryClient();
  const [error, setError] = useState<string>();
  const mutation = useMutation({ mutationFn: bootstrapAdmin });
  const form = useForm({
    defaultValues: { token: "", email: "", password: "" },
    onSubmit: async ({ value }) => {
      setError(undefined);
      const parsed = setupSchema.safeParse(value);
      if (!parsed.success) return setError(parsed.error.issues[0]?.message);
      try {
        await mutation.mutateAsync(parsed.data);
        await queryClient.invalidateQueries({ queryKey: ["auth"] });
      } catch (reason) {
        setError(errorMessage(reason));
      }
    },
  });
  return (
    <AuthFrame>
      <p className="step-label">01 / INITIALIZE</p>
      <h2>Create the first admin</h2>
      <p className="panel-copy">
        Run <code>depot setup</code> on the server, then paste its short-lived
        token here.
      </p>
      <form
        onSubmit={(event) => {
          event.preventDefault();
          void form.handleSubmit();
        }}
      >
        <form.Field name="token">
          {(field) => (
            <LabeledInput
              label="Setup token"
              type="password"
              value={field.state.value}
              onBlur={field.handleBlur}
              onChange={field.handleChange}
              autoComplete="one-time-code"
            />
          )}
        </form.Field>
        <form.Field name="email">
          {(field) => (
            <LabeledInput
              label="Admin email"
              type="email"
              value={field.state.value}
              onBlur={field.handleBlur}
              onChange={field.handleChange}
              autoComplete="email"
            />
          )}
        </form.Field>
        <form.Field name="password">
          {(field) => (
            <LabeledInput
              label="Password"
              type="password"
              value={field.state.value}
              onBlur={field.handleBlur}
              onChange={field.handleChange}
              autoComplete="new-password"
            />
          )}
        </form.Field>
        {error && (
          <p className="notice danger" role="alert">
            {error}
          </p>
        )}
        <button
          className="primary-action"
          type="submit"
          disabled={mutation.isPending}
        >
          Initialize Depot →
        </button>
      </form>
    </AuthFrame>
  );
}

/** Authenticates a local user and refreshes session-backed query state. */
function LoginForm() {
  const queryClient = useQueryClient();
  const [error, setError] = useState<string>();
  const mutation = useMutation({ mutationFn: login });
  const form = useForm({
    defaultValues: { email: "", password: "" },
    onSubmit: async ({ value }) => {
      setError(undefined);
      const parsed = loginSchema.safeParse(value);
      if (!parsed.success) return setError(parsed.error.issues[0]?.message);
      try {
        await mutation.mutateAsync(parsed.data);
        await queryClient.invalidateQueries({
          queryKey: ["auth", "current-user"],
        });
      } catch (reason) {
        setError(errorMessage(reason));
      }
    },
  });
  return (
    <AuthFrame>
      <p className="step-label">ACCESS / CONTROL PLANE</p>
      <h2>Welcome back</h2>
      <p className="panel-copy">
        Use your local Depot account. Sessions remain on this instance.
      </p>
      <form
        onSubmit={(event) => {
          event.preventDefault();
          void form.handleSubmit();
        }}
      >
        <form.Field name="email">
          {(field) => (
            <LabeledInput
              label="Email"
              type="email"
              value={field.state.value}
              onBlur={field.handleBlur}
              onChange={field.handleChange}
              autoComplete="email"
            />
          )}
        </form.Field>
        <form.Field name="password">
          {(field) => (
            <LabeledInput
              label="Password"
              type="password"
              value={field.state.value}
              onBlur={field.handleBlur}
              onChange={field.handleChange}
              autoComplete="current-password"
            />
          )}
        </form.Field>
        {error && (
          <p className="notice danger" role="alert">
            {error}
          </p>
        )}
        <button
          className="primary-action"
          type="submit"
          disabled={mutation.isPending}
        >
          Sign in →
        </button>
      </form>
    </AuthFrame>
  );
}

/** Renders an accessible controlled text field shared by authentication forms. */
function LabeledInput({
  label,
  type,
  value,
  autoComplete,
  onBlur,
  onChange,
}: {
  label: string;
  type: string;
  value: string;
  autoComplete: string;
  onBlur: () => void;
  onChange: (value: string) => void;
}) {
  return (
    <label className="field">
      <span>{label}</span>
      <input
        type={type}
        value={value}
        autoComplete={autoComplete}
        onBlur={onBlur}
        onChange={(event) => onChange(event.target.value)}
      />
    </label>
  );
}

/** Shows the authenticated identity while the operational dashboard is built. */
function Dashboard({
  user,
  location,
}: {
  user: User;
  location: DashboardLocation;
}) {
  const queryClient = useQueryClient();
  const readiness = useQuery({
    queryKey: ["system", "readiness"],
    queryFn: getSystemReadiness,
    refetchInterval: 30_000,
  });
  const mutation = useMutation({
    mutationFn: logout,
    onSuccess: () =>
      queryClient.invalidateQueries({ queryKey: ["auth", "current-user"] }),
  });
  return (
    <main className="app-shell">
      <aside className="app-sidebar">
        <div className="sidebar-brand">
          <span className="brand-word">depot_</span>
        </div>
        <div className="workspace-switcher">
          <span>
            <small>Workspace</small>
            <strong>Personal workspace</strong>
          </span>
          <i aria-hidden="true" />
        </div>
        <span className="nav-section-label">Control plane</span>
        <nav className="app-navigation" aria-label="Workspace">
          <Link
            className={
              location.kind === "projects" || location.kind === "project"
                ? "active"
                : undefined
            }
            to="/projects"
          >
            <NavigationIcon name="projects" />
            Projects
          </Link>
          <Link
            className={location.kind === "templates" ? "active" : undefined}
            to="/templates"
          >
            <NavigationIcon name="templates" />
            Templates
          </Link>
          {user.role === "admin" && (
            <Link
              className={location.kind === "team" ? "active" : undefined}
              to="/team"
            >
              <NavigationIcon name="team" />
              Team
            </Link>
          )}
        </nav>
        <div className="sidebar-footer">
          <span className="system-state">
            <i
              className={readiness.isSuccess ? "ready" : undefined}
              aria-hidden="true"
            />
            {readiness.isSuccess
              ? "Control plane ready"
              : "Checking control plane"}
          </span>
          <span className="sidebar-version">Depot v0.4.3</span>
        </div>
      </aside>
      <div className="app-workspace">
        <header className="app-header">
          <div className="breadcrumb">
            <span>Personal workspace</span>
            <b>/</b>
            <strong>{pageTitle(location)}</strong>
            {location.kind === "project" && (
              <>
                <b>/</b>
                <strong className="breadcrumb-current">
                  {location.tab.slice(0, 1).toUpperCase() +
                    location.tab.slice(1)}
                </strong>
              </>
            )}
          </div>
          <div className="account-menu">
            <span className="account-avatar">
              {user.email.slice(0, 1).toUpperCase()}
            </span>
            <span className="account-copy">
              <strong>{user.email}</strong>
              <small>{user.role}</small>
            </span>
            <button
              className="quiet-button"
              onClick={() => mutation.mutate()}
              type="button"
            >
              Sign out
            </button>
          </div>
        </header>
        <div className="app-content">
          {location.kind !== "project" && (
            <section className="page-intro">
              <div>
                <p className="eyebrow">
                  {location.kind === "projects"
                    ? "// Deployments"
                    : location.kind === "team"
                      ? "// Access"
                      : "// Architecture"}
                </p>
                <h1>{pageTitle(location)}</h1>
                <p>{pageDescription(location)}</p>
              </div>
              <span className="control-plane-status">
                <i
                  className={readiness.isSuccess ? "ready" : undefined}
                  aria-hidden="true"
                />
                {readiness.isSuccess
                  ? "Control plane ready"
                  : "Status unavailable"}
              </span>
            </section>
          )}
          {location.kind === "templates" ? (
            <TemplatesPanel />
          ) : location.kind === "team" ? (
            user.role === "admin" ? (
              <UsersPanel />
            ) : (
              <section className="project-not-found">
                <span>403</span>
                <h2>Administrator access required</h2>
                <p>Your account cannot manage workspace members.</p>
                <Link to="/projects">Return to projects</Link>
              </section>
            )
          ) : (
            <ProjectsPanel
              projectId={
                location.kind === "project" ? location.projectId : undefined
              }
              tab={location.kind === "project" ? location.tab : undefined}
              deploymentId={
                location.kind === "project" ? location.deploymentId : undefined
              }
            />
          )}
        </div>
      </div>
    </main>
  );
}

/** Small dependency-free navigation glyphs with currentColor styling. */
function NavigationIcon({ name }: { name: "projects" | "team" | "templates" }) {
  if (name === "projects")
    return (
      <svg aria-hidden="true" viewBox="0 0 20 20">
        <path d="M3.5 4.5h5v5h-5zM11.5 4.5h5v5h-5zM3.5 12.5h5v3h-5zM11.5 12.5h5v3h-5z" />
      </svg>
    );
  if (name === "templates")
    return (
      <svg aria-hidden="true" viewBox="0 0 20 20">
        <path d="M3.5 3.5h5.5v5.5H3.5zM11 3.5h5.5v5.5H11zM3.5 11h5.5v5.5H3.5zM11 11h5.5v5.5H11z" />
      </svg>
    );
  return (
    <svg aria-hidden="true" viewBox="0 0 20 20">
      <path d="M7.5 9a3 3 0 1 0 0-6 3 3 0 0 0 0 6ZM2.5 16.5v-2a3 3 0 0 1 3-3h4a3 3 0 0 1 3 3v2M13 8.5a2.5 2.5 0 1 0 0-5M13.5 11.5h1a3 3 0 0 1 3 3v2" />
    </svg>
  );
}

/** Returns a stable heading for every URL-backed workspace page. */
function pageTitle(location: DashboardLocation) {
  if (location.kind === "team") return "Team";
  if (location.kind === "templates") return "Templates";
  return "Projects";
}

/** Describes the job of each top-level workspace page. */
function pageDescription(location: DashboardLocation) {
  if (location.kind === "team")
    return "Manage the people who can access this Depot instance.";
  if (location.kind === "templates")
    return "Start with safe defaults or follow a dependency-aware application blueprint.";
  return "Connect source, configure builds, and promote releases from one place.";
}

/** Converts unknown failures into useful but non-sensitive UI copy. */
function errorMessage(reason: unknown) {
  return reason instanceof ApiError
    ? reason.message
    : "Depot could not complete the request.";
}
