import { z } from "zod";

const errorSchema = z.object({
  error: z.object({ code: z.string(), message: z.string() }),
});
const bootstrapStatusSchema = z.object({ initialized: z.boolean() });
const readinessSchema = z.object({
  status: z.literal("ready"),
  database: z.literal("ready"),
});
const userSchema = z.object({
  id: z.string(),
  email: z.email(),
  role: z.enum(["admin", "member"]),
});
const loginResponseSchema = z.discriminatedUnion("status", [
  z.object({ status: z.literal("authenticated"), user: userSchema }),
  z.object({ status: z.literal("two_factor_required") }),
]);
const twoFactorStatusSchema = z.object({
  enabled: z.boolean(),
  recovery_codes_remaining: z.number().int().nonnegative(),
});
const twoFactorEnrollmentSchema = z.object({
  secret: z.string(),
  otpauth_uri: z.string(),
  qr_base64: z.string(),
});
const recoveryCodesSchema = z.object({
  recovery_codes: z.array(z.string()),
});
const adminUserSchema = userSchema.extend({
  active: z.boolean(),
  created_at: z.number(),
});
const projectSchema = z.object({
  id: z.string(),
  slug: z.string(),
  name: z.string(),
  owner_user_id: z.string(),
  repository_url: z.string(),
  branch: z.string(),
  created_at: z.number(),
  updated_at: z.number(),
  latest_inspection_job_id: z.string().nullable(),
});
const serviceConfigSchema = z.object({
  project_id: z.string(),
  project_kind: z.enum(["auto", "service", "static"]),
  builder: z.enum(["auto", "dockerfile", "railpack"]),
  context_path: z.string(),
  dockerfile_path: z.string(),
  static_output_dir: z.string().nullable(),
  exposed_port: z.number().int().nullable(),
  healthcheck_path: z.string().nullable(),
  updated_at: z.number(),
});
const inspectionResultSchema = z.object({
  commit_sha: z.string(),
  builder: z.enum(["dockerfile", "railpack", "static", "unknown"]),
  reason: z.string(),
  has_mise_config: z.boolean(),
  exposed_ports: z.array(z.number().int()).default([]),
  inferred_port: z.number().int().optional(),
  initial_deployment_id: z.string().optional(),
});
const dockerBuildResultSchema = z.object({
  image_id: z.string(),
  image_tag: z.string(),
  container_id: z.string(),
  host_port: z.number().int(),
  logs: z.string(),
});
const staticBuildResultSchema = z.object({
  static_release: z.string(),
  logs: z.string(),
});
const runtimeLogsSchema = z.object({
  status: z.enum(["available", "not_deployed", "static"]),
  deployment_id: z.string().nullable(),
  logs: z.string(),
  fetched_at: z.number(),
});
const runtimeActionResultSchema = z.object({
  action: z.enum(["start", "stop", "restart"]),
  deployment_id: z.string(),
  container_id: z.string(),
  state: z.enum(["running", "stopped"]),
});
const jobStatusSchema = z.object({
  id: z.string(),
  kind: z.string(),
  project_id: z.string().nullable(),
  state: z.enum(["queued", "running", "succeeded", "failed"]),
  attempts: z.number(),
  max_attempts: z.number(),
  result: z
    .union([
      inspectionResultSchema,
      dockerBuildResultSchema,
      staticBuildResultSchema,
      runtimeLogsSchema,
      runtimeActionResultSchema,
    ])
    .nullable(),
  last_error: z.string().nullable(),
  created_at: z.number(),
  updated_at: z.number(),
  events: z.array(
    z.object({
      sequence: z.number(),
      level: z.enum(["info", "warning", "error"]),
      message: z.string(),
      created_at: z.number(),
    }),
  ),
});
const deploymentSchema = z.object({
  id: z.string(),
  project_id: z.string(),
  job_id: z.string(),
  commit_sha: z.string(),
  trigger_kind: z.enum(["manual", "webhook"]),
  status: z.enum(["queued", "building", "succeeded", "failed"]),
  failure_reason: z.string().nullable(),
  image_id: z.string().nullable(),
  container_id: z.string().nullable(),
  host_port: z.number().int().nullable(),
  runtime_route_status: z.enum(["pending", "verified", "error"]),
  runtime_route_error: z.string().nullable(),
  runtime_route_checked_at: z.number().int().nullable(),
  runtime_desired_state: z.enum(["running", "stopped"]),
  created_at: z.number(),
  updated_at: z.number(),
  finished_at: z.number().nullable(),
  active: z.boolean(),
});
const domainSchema = z.object({
  id: z.string(),
  project_id: z.string(),
  hostname: z.string(),
  routing_status: z.enum(["pending", "active", "error"]),
  last_error: z.string().nullable(),
  created_at: z.number(),
  updated_at: z.number(),
});
const variableSchema = z.object({
  id: z.string(),
  project_id: z.string(),
  name: z.string(),
  is_secret: z.boolean(),
  created_at: z.number(),
  updated_at: z.number(),
});
const containerFileSchema = z.object({
  name: z.string(),
  kind: z.enum(["file", "directory", "symlink"]),
  size: z.number().nonnegative(),
});
const githubWebhookCredentialSchema = z.object({
  endpoint_path: z.string(),
  secret: z.string(),
});
const githubInstallationSchema = z.object({
  installation_id: z.number().int().positive(),
  account_login: z.string(),
  account_type: z.string(),
  repository_selection: z.string(),
});
const githubIntegrationSchema = z.object({
  configured: z.boolean(),
  install_url: z.string().url().nullable(),
  setup_available: z.boolean(),
  manifest_url: z.string().url().nullable(),
  manifest: z.string().nullable(),
  installations: z.array(githubInstallationSchema),
});
const githubRepositorySchema = z.object({
  id: z.number().int().positive(),
  name: z.string(),
  full_name: z.string(),
  private: z.boolean(),
  clone_url: z.string().url(),
  default_branch: z.string(),
  archived: z.boolean(),
});
const githubRepositoryEntrySchema = z
  .union([
    z.object({ name: z.string(), path: z.string(), kind: z.string() }),
    z.object({ name: z.string(), path: z.string(), type: z.string() }),
  ])
  .transform((entry) => ({
    name: entry.name,
    path: entry.path,
    kind: "kind" in entry ? entry.kind : entry.type,
  }));
const projectGithubSourceSchema = z.object({
  installation_id: z.number().int().positive(),
  repository_id: z.number().int().positive(),
  repository_full_name: z.string(),
  branch: z.string(),
});
const sshCredentialResponseSchema = z.object({ configured: z.literal(true) });

export type User = z.infer<typeof userSchema>;
export type AdminUser = z.infer<typeof adminUserSchema>;
export type Project = z.infer<typeof projectSchema>;
export type JobStatus = z.infer<typeof jobStatusSchema>;
export type ServiceConfig = z.infer<typeof serviceConfigSchema>;
export type ServiceConfigInput = Omit<
  ServiceConfig,
  "project_id" | "updated_at"
>;
export type Deployment = z.infer<typeof deploymentSchema>;
export type Domain = z.infer<typeof domainSchema>;
export type ProjectVariable = z.infer<typeof variableSchema>;
export type GithubIntegration = z.infer<typeof githubIntegrationSchema>;
export type GithubRepository = z.infer<typeof githubRepositorySchema>;
export type GithubRepositoryEntry = z.infer<typeof githubRepositoryEntrySchema>;
export type ProjectGithubSource = z.infer<typeof projectGithubSourceSchema>;

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

/** Verifies that the control plane and its database are ready for requests. */
export function getSystemReadiness() {
  return request("/api/v1/system/readiness", readinessSchema);
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
export function login(input: {
  email: string;
  password: string;
  code?: string;
}) {
  return request("/api/v1/auth/login", loginResponseSchema, {
    method: "POST",
    body: JSON.stringify(input),
  });
}

/** Returns the signed-in account's non-secret 2FA state. */
export function getTwoFactorStatus() {
  return request("/api/v1/auth/2fa", twoFactorStatusSchema);
}

/** Starts a replaceable TOTP enrollment and returns its one-time setup material. */
export function beginTwoFactorEnrollment() {
  return request("/api/v1/auth/2fa/enroll", twoFactorEnrollmentSchema, {
    method: "POST",
    headers: csrfHeaders(),
  });
}

/** Confirms a pending TOTP enrollment and returns recovery codes once. */
export function confirmTwoFactorEnrollment(code: string) {
  return request("/api/v1/auth/2fa/confirm", recoveryCodesSchema, {
    method: "POST",
    headers: csrfHeaders(),
    body: JSON.stringify({ code }),
  });
}

/** Replaces all unused recovery codes after full reauthentication. */
export function regenerateTwoFactorRecoveryCodes(input: {
  current_password: string;
  code: string;
}) {
  return request("/api/v1/auth/2fa/recovery-codes", recoveryCodesSchema, {
    method: "POST",
    headers: csrfHeaders(),
    body: JSON.stringify(input),
  });
}

/** Disables 2FA after password and second-factor verification. */
export async function disableTwoFactor(input: {
  current_password: string;
  code: string;
}) {
  const response = await fetch("/api/v1/auth/2fa", {
    method: "DELETE",
    credentials: "same-origin",
    headers: { "Content-Type": "application/json", ...csrfHeaders() },
    body: JSON.stringify(input),
  });
  if (!response.ok) await throwFileResponseError(response);
}

/** Returns the identity associated with the current session cookie. */
export function getCurrentUser() {
  return request("/api/v1/auth/me", userSchema);
}

/** Changes the signed-in account password and expires every existing session. */
export function changePassword(input: {
  current_password: string;
  new_password: string;
}) {
  return request(
    "/api/v1/auth/password",
    z.object({ changed: z.literal(true) }),
    {
      method: "PUT",
      headers: csrfHeaders(),
      body: JSON.stringify(input),
    },
  );
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

/** Converts a failed non-JSON file response into the standard client error. */
async function throwFileResponseError(response: Response): Promise<never> {
  const body: unknown = await response.json().catch(() => undefined);
  const parsed = errorSchema.safeParse(body);
  throw parsed.success
    ? new ApiError(parsed.data.error.code, parsed.data.error.message)
    : new ApiError(
        "container_file_failed",
        "Depot could not complete that container file operation.",
      );
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

/** Lists projects visible in the shared v0.1 workspace. */
export function listProjects() {
  return request("/api/v1/projects", z.array(projectSchema));
}

/** Creates a project from validated Git repository metadata. */
export function createProject(input: {
  slug: string;
  name: string;
  repository_url: string;
  branch: string;
}) {
  return request("/api/v1/projects", projectSchema, {
    method: "POST",
    headers: csrfHeaders(),
    body: JSON.stringify(input),
  });
}

/** Updates mutable source metadata for an existing project. */
export function updateProject(
  projectId: string,
  input: { name: string; repository_url: string; branch: string },
) {
  return request(`/api/v1/projects/${projectId}`, projectSchema, {
    method: "PATCH",
    headers: csrfHeaders(),
    body: JSON.stringify(input),
  });
}

/** Permanently removes a project after server-side dependency checks. */
export async function deleteProject(projectId: string) {
  const response = await fetch(`/api/v1/projects/${projectId}`, {
    method: "DELETE",
    credentials: "same-origin",
    headers: csrfHeaders(),
  });
  if (!response.ok) {
    const body: unknown = await response.json().catch(() => undefined);
    const parsed = errorSchema.safeParse(body);
    throw parsed.success
      ? new ApiError(parsed.data.error.code, parsed.data.error.message)
      : new ApiError(
          "project_delete_failed",
          "Depot could not remove this project.",
        );
  }
}

/** Returns configured GitHub App state and installations linked to Depot. */
export function getGithubIntegration() {
  return request("/api/v1/integrations/github", githubIntegrationSchema);
}

/** Completes GitHub's one-time manifest exchange and stores the generated App securely. */
export function completeGithubManifest(code: string) {
  return request(
    "/api/v1/integrations/github/manifest",
    githubIntegrationSchema,
    {
      method: "POST",
      headers: csrfHeaders(),
      body: JSON.stringify({ code }),
    },
  );
}

/** Verifies and links the installation returned by GitHub's setup callback. */
export function linkGithubInstallation(installationId: number) {
  return request(
    "/api/v1/integrations/github/installations",
    githubInstallationSchema,
    {
      method: "POST",
      headers: csrfHeaders(),
      body: JSON.stringify({ installation_id: installationId }),
    },
  );
}

/** Lists repositories visible to one linked GitHub App installation. */
export function listGithubRepositories(installationId: number) {
  return request(
    `/api/v1/integrations/github/installations/${installationId}/repositories`,
    z.array(githubRepositorySchema),
  );
}

/** Lists one directory in a GitHub repository using Depot's private App credential. */
export function listGithubRepositoryDirectory(
  installationId: number,
  repositoryId: number,
  reference: string,
  path: string,
) {
  const query = new URLSearchParams({ ref: reference, path });
  return request(
    `/api/v1/integrations/github/installations/${installationId}/repositories/${repositoryId}/contents?${query}`,
    z.array(githubRepositoryEntrySchema),
  );
}

/** Creates a Depot project from a server-verified GitHub repository. */
export function importGithubProject(input: {
  installation_id: number;
  repository_id: number;
  slug: string;
  name: string;
  branch?: string;
}) {
  return request("/api/v1/integrations/github/projects", projectSchema, {
    method: "POST",
    headers: csrfHeaders(),
    body: JSON.stringify(input),
  });
}

/** Enqueues deduplicated repository inspection for the current project revision. */
export function inspectProject(projectId: string) {
  return request(
    `/api/v1/projects/${projectId}/inspect`,
    z.object({ job_id: z.string() }),
    { method: "POST", headers: csrfHeaders() },
  );
}

/** Inspects the current branch and durably deploys its resolved latest commit. */
export function launchProject(projectId: string) {
  return request(
    `/api/v1/projects/${projectId}/launch`,
    z.object({ job_id: z.string() }),
    { method: "POST", headers: csrfHeaders() },
  );
}

/** Returns GitHub source metadata when Depot can browse a project's repository. */
export function getProjectGithubSource(projectId: string) {
  return request(
    `/api/v1/projects/${projectId}/github-source`,
    projectGithubSourceSchema.nullable(),
  );
}

/** Returns durable state, structured inspection result, and ordered events. */
export function getJobStatus(jobId: string) {
  return request(`/api/v1/jobs/${jobId}`, jobStatusSchema);
}

/** Loads the newest durable service lifecycle operation for one project. */
export function getLatestRuntimeAction(projectId: string) {
  return request(
    `/api/v1/projects/${projectId}/runtime-action`,
    jobStatusSchema.nullable(),
  );
}

/** Queues a fixed lifecycle operation for the active service container. */
export function controlProjectRuntime(
  projectId: string,
  action: "start" | "stop" | "restart",
) {
  return request(
    `/api/v1/projects/${projectId}/runtime/${action}`,
    z.object({ job_id: z.string() }),
    { method: "POST", headers: csrfHeaders() },
  );
}

/** Loads one project's persisted build and runtime settings. */
export function getServiceConfig(projectId: string) {
  return request(`/api/v1/projects/${projectId}/config`, serviceConfigSchema);
}

/** Replaces one project's build and runtime settings after CSRF validation. */
export function updateServiceConfig(
  projectId: string,
  input: ServiceConfigInput,
) {
  return request(`/api/v1/projects/${projectId}/config`, serviceConfigSchema, {
    method: "PUT",
    headers: csrfHeaders(),
    body: JSON.stringify(input),
  });
}

/** Lists durable deployment attempts for one project from newest to oldest. */
export function listDeployments(projectId: string) {
  return request(
    `/api/v1/projects/${projectId}/deployments`,
    z.array(deploymentSchema),
  );
}

/** Enqueues bounded active-container output collection in the worker. */
export function requestRuntimeLogs(projectId: string) {
  return request(
    `/api/v1/projects/${projectId}/runtime-logs`,
    z.object({ job_id: z.string() }),
    { method: "POST", headers: csrfHeaders() },
  );
}

/** Lists direct children of one absolute directory in the active container. */
export function listContainerFiles(projectId: string, path: string) {
  const query = new URLSearchParams({ path });
  return request(
    `/api/v1/projects/${projectId}/container-files?${query}`,
    z.array(containerFileSchema),
  );
}

/** Uploads one browser file into the active container with CSRF protection. */
export async function uploadContainerFile(
  projectId: string,
  path: string,
  file: File,
) {
  const query = new URLSearchParams({ path });
  const response = await fetch(
    `/api/v1/projects/${projectId}/container-files?${query}`,
    {
      method: "PUT",
      credentials: "same-origin",
      headers: csrfHeaders(),
      body: file,
    },
  );
  if (!response.ok) await throwFileResponseError(response);
}

/** Downloads one regular file from the active container as a browser blob. */
export async function downloadContainerFile(projectId: string, path: string) {
  const query = new URLSearchParams({ path });
  const response = await fetch(
    `/api/v1/projects/${projectId}/container-files/download?${query}`,
    { credentials: "same-origin" },
  );
  if (!response.ok) await throwFileResponseError(response);
  return response.blob();
}

/** Creates one directory in the active container. */
export async function createContainerDirectory(
  projectId: string,
  path: string,
) {
  const response = await fetch(
    `/api/v1/projects/${projectId}/container-files/directory`,
    {
      method: "POST",
      credentials: "same-origin",
      headers: { "Content-Type": "application/json", ...csrfHeaders() },
      body: JSON.stringify({ path }),
    },
  );
  if (!response.ok) await throwFileResponseError(response);
}

/** Moves or renames one file-system entry in the active container. */
export async function renameContainerFile(
  projectId: string,
  path: string,
  destination: string,
) {
  const response = await fetch(
    `/api/v1/projects/${projectId}/container-files/rename`,
    {
      method: "POST",
      credentials: "same-origin",
      headers: { "Content-Type": "application/json", ...csrfHeaders() },
      body: JSON.stringify({ path, destination }),
    },
  );
  if (!response.ok) await throwFileResponseError(response);
}

/** Recursively removes one explicitly selected non-root container path. */
export async function deleteContainerFile(projectId: string, path: string) {
  const query = new URLSearchParams({ path });
  const response = await fetch(
    `/api/v1/projects/${projectId}/container-files?${query}`,
    {
      method: "DELETE",
      credentials: "same-origin",
      headers: csrfHeaders(),
    },
  );
  if (!response.ok) await throwFileResponseError(response);
}

/** Enqueues a manual deployment of the latest successfully inspected commit. */
export function createDeployment(projectId: string) {
  return request(
    `/api/v1/projects/${projectId}/deployments`,
    deploymentSchema,
    {
      method: "POST",
      headers: csrfHeaders(),
    },
  );
}

/** Creates or rotates one signed GitHub push webhook and returns credentials once. */
export function createGithubWebhook(
  projectId: string,
  repositoryFullName: string,
) {
  return request(
    `/api/v1/projects/${projectId}/github-webhook`,
    githubWebhookCredentialSchema,
    {
      method: "PUT",
      headers: csrfHeaders(),
      body: JSON.stringify({ repository_full_name: repositoryFullName }),
    },
  );
}

/** Encrypts or rotates a write-only SSH deploy key and explicit host trust. */
export function upsertSshCredential(
  projectId: string,
  input: { private_key: string; known_hosts: string },
) {
  return request(
    `/api/v1/projects/${projectId}/ssh-credential`,
    sshCredentialResponseSchema,
    {
      method: "PUT",
      headers: csrfHeaders(),
      body: JSON.stringify(input),
    },
  );
}

/** Rebuilds a retained successful commit through the normal promotion path. */
export function rollbackDeployment(deploymentId: string) {
  return request(
    `/api/v1/deployments/${deploymentId}/rollback`,
    deploymentSchema,
    { method: "POST", headers: csrfHeaders() },
  );
}

/** Lists hostnames attached to one project. */
export function listDomains(projectId: string) {
  return request(
    `/api/v1/projects/${projectId}/domains`,
    z.array(domainSchema),
  );
}

/** Attaches one validated public hostname to a project. */
export function createDomain(projectId: string, hostname: string) {
  return request(`/api/v1/projects/${projectId}/domains`, domainSchema, {
    method: "POST",
    headers: csrfHeaders(),
    body: JSON.stringify({ hostname }),
  });
}

/** Removes one project hostname after CSRF validation. */
export async function deleteDomain(domainId: string) {
  const response = await fetch(`/api/v1/domains/${domainId}`, {
    method: "DELETE",
    credentials: "same-origin",
    headers: csrfHeaders(),
  });
  if (!response.ok) {
    const body: unknown = await response.json().catch(() => undefined);
    const parsed = errorSchema.safeParse(body);
    throw parsed.success
      ? new ApiError(parsed.data.error.code, parsed.data.error.message)
      : new ApiError("delete_failed", "Depot could not remove this domain.");
  }
}

/** Requeues the complete desired-domain snapshot for the restricted Caddy agent. */
export function retryDomainReconciliation() {
  return request(
    "/api/v1/domains/reconcile",
    z.object({ job_id: z.string() }),
    { method: "POST", headers: csrfHeaders() },
  );
}

/** Lists masked runtime-variable metadata without loading stored values. */
export function listVariables(projectId: string) {
  return request(
    `/api/v1/projects/${projectId}/variables`,
    z.array(variableSchema),
  );
}

/** Encrypts and creates or replaces one write-only runtime value. */
export function upsertVariable(
  projectId: string,
  input: { name: string; value: string; is_secret: boolean },
) {
  return request(`/api/v1/projects/${projectId}/variables`, variableSchema, {
    method: "PUT",
    headers: csrfHeaders(),
    body: JSON.stringify(input),
  });
}

/** Deletes one encrypted runtime value after CSRF validation. */
export async function deleteVariable(projectId: string, variableId: string) {
  const response = await fetch(
    `/api/v1/projects/${projectId}/variables/${variableId}`,
    { method: "DELETE", credentials: "same-origin", headers: csrfHeaders() },
  );
  if (!response.ok) {
    const body: unknown = await response.json().catch(() => undefined);
    const parsed = errorSchema.safeParse(body);
    throw parsed.success
      ? new ApiError(parsed.data.error.code, parsed.data.error.message)
      : new ApiError(
          "variable_delete_failed",
          "Depot could not delete the value.",
        );
  }
}
