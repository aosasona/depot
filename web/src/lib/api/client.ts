import { z } from "zod";

const errorSchema = z.object({
  error: z.object({ code: z.string(), message: z.string() }),
});
const bootstrapStatusSchema = z.object({ initialized: z.boolean() });
const userSchema = z.object({
  id: z.string(),
  email: z.email(),
  role: z.enum(["admin", "member"]),
});
const adminUserSchema = userSchema.extend({
  active: z.boolean(),
  created_at: z.number(),
});

export type User = z.infer<typeof userSchema>;
export type AdminUser = z.infer<typeof adminUserSchema>;

/** Represents a safe API failure suitable for inline UI feedback. */
export class ApiError extends Error {
  /** Creates an API error with its stable machine-readable code. */
  constructor(
    public readonly code: string,
    message: string,
  ) {
    super(message);
    this.name = "ApiError";
  }
}

/** Fetches JSON and validates both success and failure payloads. */
async function request<T>(
  path: string,
  schema: z.ZodType<T>,
  init?: RequestInit,
) {
  const response = await fetch(path, {
    credentials: "same-origin",
    ...init,
    headers: { "Content-Type": "application/json", ...init?.headers },
  });
  const body: unknown = await response.json();
  if (!response.ok) {
    const parsed = errorSchema.safeParse(body);
    throw parsed.success
      ? new ApiError(parsed.data.error.code, parsed.data.error.message)
      : new ApiError(
          "invalid_response",
          "Depot returned an unexpected response.",
        );
  }
  return schema.parse(body);
}

/** Returns whether this instance has completed first-admin setup. */
export function getBootstrapStatus() {
  return request("/api/v1/auth/bootstrap", bootstrapStatusSchema);
}

/** Consumes the operator token and creates the first local administrator. */
export function bootstrapAdmin(input: {
  token: string;
  email: string;
  password: string;
}) {
  return request("/api/v1/auth/bootstrap", z.object({ user_id: z.string() }), {
    method: "POST",
    body: JSON.stringify(input),
  });
}

/** Creates a revocable browser session using local account credentials. */
export function login(input: { email: string; password: string }) {
  return request("/api/v1/auth/login", userSchema, {
    method: "POST",
    body: JSON.stringify(input),
  });
}

/** Returns the identity associated with the current session cookie. */
export function getCurrentUser() {
  return request("/api/v1/auth/me", userSchema);
}

/** Reads one non-HttpOnly cookie by exact name. */
function readCookie(name: string) {
  const prefix = `${name}=`;
  return document.cookie
    .split(";")
    .map((part) => part.trim())
    .find((part) => part.startsWith(prefix))
    ?.slice(prefix.length);
}

/** Returns the session-bound CSRF header required by authenticated mutations. */
function csrfHeaders() {
  const csrfToken = readCookie("depot_csrf");
  if (!csrfToken)
    throw new ApiError("missing_csrf", "The session cannot be verified.");
  return { "x-csrf-token": csrfToken };
}

/** Revokes the active session using the CSRF value bound to it. */
export async function logout() {
  const response = await fetch("/api/v1/auth/logout", {
    method: "POST",
    credentials: "same-origin",
    headers: csrfHeaders(),
  });
  if (!response.ok)
    throw new ApiError("logout_failed", "Depot could not sign you out.");
}

/** Lists all local users for an authenticated administrator. */
export function listUsers() {
  return request("/api/v1/users", z.array(adminUserSchema));
}

/** Creates a local account with an administrator-selected initial password. */
export function createUser(input: {
  email: string;
  password: string;
  role: "admin" | "member";
}) {
  return request("/api/v1/users", adminUserSchema, {
    method: "POST",
    headers: csrfHeaders(),
    body: JSON.stringify(input),
  });
}

/** Updates one user's role and activation state. */
export function updateUser(input: {
  id: string;
  role: "admin" | "member";
  active: boolean;
}) {
  return request(`/api/v1/users/${input.id}`, adminUserSchema, {
    method: "PATCH",
    headers: csrfHeaders(),
    body: JSON.stringify({ role: input.role, active: input.active }),
  });
}
