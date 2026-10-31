import { useForm } from "@tanstack/react-form";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { type ReactNode, useState } from "react";
import { z } from "zod";
import {
  ApiError,
  bootstrapAdmin,
  login,
  logout,
  type User,
} from "../../lib/api/client";
import { ProjectsPanel } from "../projects/ProjectsPanel";
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

/** Selects setup, login, or dashboard from real API state. */
export function AuthEntryPage() {
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
  if (session.data) return <Dashboard user={session.data} />;
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
function Dashboard({ user }: { user: User }) {
  const queryClient = useQueryClient();
  const mutation = useMutation({
    mutationFn: logout,
    onSuccess: () =>
      queryClient.invalidateQueries({ queryKey: ["auth", "current-user"] }),
  });
  return (
    <main className="shell">
      <header className="masthead">
        <span className="brand">
          DEPOT<span>_</span>
        </span>
        <button
          className="text-action"
          onClick={() => mutation.mutate()}
          type="button"
        >
          Sign out
        </button>
      </header>
      <section className="hero">
        <p className="eyebrow">CONTROL PLANE ONLINE</p>
        <h1>Ready to deploy.</h1>
        <p className="lede">
          Signed in as {user.email} / {user.role}
        </p>
        <div className="status-panel">
          <span className="status-dot" />
          <span>Authentication operational</span>
          <code>milestone/a</code>
        </div>
      </section>
      <ProjectsPanel />
      {user.role === "admin" && <UsersPanel />}
    </main>
  );
}

/** Converts unknown failures into useful but non-sensitive UI copy. */
function errorMessage(reason: unknown) {
  return reason instanceof ApiError
    ? reason.message
    : "Depot could not complete the request.";
}
