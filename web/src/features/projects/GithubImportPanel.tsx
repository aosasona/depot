import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useMemo, useRef, useState } from "react";
import {
  ApiError,
  completeGithubManifest,
  createProject,
  type GithubRepository,
  getGithubIntegration,
  importGithubProject,
  linkGithubInstallation,
  listGithubRepositories,
  type Project,
  updateServiceConfig,
} from "../../lib/api/client";
import { RepositoryDirectoryPicker } from "./RepositoryDirectoryPicker";

type Source =
  | { kind: "github"; repository: GithubRepository }
  | { kind: "manual" };
type ConfigureValues = {
  name: string;
  slug: string;
  branch: string;
  repositoryUrl: string;
  contextPath: string;
  projectKind: "auto" | "service" | "static";
  builder: "auto" | "dockerfile" | "railpack";
  staticOutput: string;
  port: string;
};
const initialValues: ConfigureValues = {
  name: "",
  slug: "",
  branch: "main",
  repositoryUrl: "",
  contextPath: ".",
  projectKind: "auto",
  builder: "auto",
  staticOutput: "dist",
  port: "",
};

/** Launches a guided GitHub-first project creation flow. */
export function GithubImportPanel() {
  const dialog = useRef<HTMLDialogElement>(null);
  const manifestCallback = useRef(false);
  const [open, setOpen] = useState(false);
  const [setupMessage, setSetupMessage] = useState<string>();
  const client = useQueryClient();
  const integration = useQuery({
    queryKey: ["github-integration"],
    queryFn: getGithubIntegration,
  });
  const template = new URLSearchParams(window.location.search).get("template");
  useEffect(() => {
    if (
      new URLSearchParams(window.location.search).has("installation_id") ||
      template
    )
      setOpen(true);
  }, [template]);
  useEffect(() => {
    if (manifestCallback.current) return;
    const params = new URLSearchParams(window.location.search);
    const code = params.get("code");
    if (!code) return;
    manifestCallback.current = true;
    setOpen(true);
    const returnedState = params.get("state");
    const expectedState = window.sessionStorage.getItem(
      "depot_github_manifest_state",
    );
    const clearCallback = () => {
      params.delete("code");
      params.delete("state");
      window.history.replaceState(
        {},
        "",
        `${window.location.pathname}${params.size ? `?${params}` : ""}`,
      );
      window.sessionStorage.removeItem("depot_github_manifest_state");
    };
    if (!expectedState || returnedState !== expectedState) {
      clearCallback();
      setSetupMessage(
        "GitHub setup could not be verified. Start the setup again from Depot.",
      );
      return;
    }
    setSetupMessage("Securing the GitHub App credentials…");
    void completeGithubManifest(code).then(
      async () => {
        clearCallback();
        setSetupMessage(
          "GitHub App created. Install it on an account to browse repositories.",
        );
        await client.invalidateQueries({ queryKey: ["github-integration"] });
      },
      (reason: unknown) => {
        clearCallback();
        setSetupMessage(errorMessage(reason));
      },
    );
  }, [client]);
  useEffect(() => {
    if (open && !dialog.current?.open) dialog.current?.showModal();
    if (!open && dialog.current?.open) dialog.current.close();
  }, [open]);
  return (
    <>
      <div className="new-project-banner">
        <div>
          <span className="eyebrow">NEW DEPLOYMENT</span>
          <strong>Ship from a repository</strong>
          <p>
            Connect GitHub or provide a Git URL. Depot detects the build and
            keeps future pushes connected.
          </p>
        </div>
        <button
          className="primary-action"
          type="button"
          onClick={() => setOpen(true)}
        >
          {integration.data?.configured ? "Browse GitHub →" : "Set up GitHub →"}
        </button>
      </div>
      <dialog
        ref={dialog}
        className="project-wizard"
        onClose={() => setOpen(false)}
        onCancel={() => setOpen(false)}
      >
        {open && (
          <ProjectWizard
            configured={integration.data?.configured ?? false}
            installUrl={integration.data?.install_url ?? null}
            setupAvailable={integration.data?.setup_available ?? false}
            manifestUrl={integration.data?.manifest_url ?? null}
            manifest={integration.data?.manifest ?? null}
            initialMessage={setupMessage}
            template={template}
            onClose={() => setOpen(false)}
          />
        )}
      </dialog>
    </>
  );
}

/** Coordinates source selection and configuration without hiding provider failures. */
function ProjectWizard({
  configured,
  installUrl,
  setupAvailable,
  manifestUrl,
  manifest,
  initialMessage,
  template,
  onClose,
}: {
  configured: boolean;
  installUrl: string | null;
  setupAvailable: boolean;
  manifestUrl: string | null;
  manifest: string | null;
  initialMessage?: string;
  template: string | null;
  onClose: () => void;
}) {
  const client = useQueryClient();
  const integration = useQuery({
    queryKey: ["github-integration"],
    queryFn: getGithubIntegration,
  });
  const [installationId, setInstallationId] = useState<number>();
  const [source, setSource] = useState<Source>();
  const [values, setValues] = useState<ConfigureValues>(initialValues);
  const [search, setSearch] = useState("");
  const [message, setMessage] = useState<string | undefined>(initialMessage);
  const [manifestState] = useState(createManifestState);
  const linkedCallback = useRef(false);
  const link = useMutation({ mutationFn: linkGithubInstallation });
  const create = useMutation({ mutationFn: createProject });
  const importProject = useMutation({ mutationFn: importGithubProject });

  useEffect(() => setMessage(initialMessage), [initialMessage]);

  useEffect(() => {
    const preset = templateDefaults(template);
    if (!preset) return;
    setSource({ kind: "manual" });
    setValues(preset);
  }, [template]);

  useEffect(() => {
    const first = integration.data?.installations[0]?.installation_id;
    if (!installationId && first) setInstallationId(first);
  }, [installationId, integration.data]);
  useEffect(() => {
    if (!configured || linkedCallback.current) return;
    const params = new URLSearchParams(window.location.search);
    const callbackId = Number(params.get("installation_id"));
    if (!Number.isSafeInteger(callbackId) || callbackId <= 0) return;
    linkedCallback.current = true;
    setMessage("Verifying GitHub installation…");
    void link.mutateAsync(callbackId).then(
      async () => {
        params.delete("installation_id");
        params.delete("setup_action");
        window.history.replaceState(
          {},
          "",
          `${window.location.pathname}${params.size ? `?${params}` : ""}`,
        );
        setInstallationId(callbackId);
        setMessage(undefined);
        await client.invalidateQueries({ queryKey: ["github-integration"] });
      },
      (reason: unknown) => setMessage(errorMessage(reason)),
    );
  }, [client, configured, link]);

  const repositories = useQuery({
    queryKey: ["github-repositories", installationId],
    queryFn: () => listGithubRepositories(installationId ?? 0),
    enabled: Boolean(installationId) && !source,
  });
  const visible = useMemo(() => {
    const needle = search.trim().toLowerCase();
    return repositories.data?.filter((repo) =>
      repo.full_name.toLowerCase().includes(needle),
    );
  }, [repositories.data, search]);

  /** Selects a verified GitHub repository and seeds editable defaults. */
  function chooseRepository(repository: GithubRepository) {
    setSource({ kind: "github", repository });
    setValues({
      ...initialValues,
      name: repository.name,
      slug: projectSlug(repository.name),
      branch: repository.default_branch,
      repositoryUrl: repository.clone_url,
    });
  }

  /** Creates the project and immediately persists its deployment configuration. */
  async function finish() {
    setMessage(undefined);
    if (!values.name.trim() || !validSlug(values.slug) || !values.branch.trim())
      return setMessage("Complete the name, valid slug, and branch.");
    if (
      !source ||
      (source.kind === "manual" &&
        !/^https?:\/\/|^ssh:\/\//.test(values.repositoryUrl))
    )
      return setMessage("Enter a valid HTTPS or SSH repository URL.");
    if (
      values.projectKind === "service" &&
      (!/^\d+$/.test(values.port) || Number(values.port) > 65535)
    )
      return setMessage("Services need a port between 1 and 65535.");
    if (!validRepositoryPath(values.contextPath))
      return setMessage("Choose a valid repository source directory.");
    try {
      let project: Project;
      if (source.kind === "github") {
        if (!installationId) throw new Error("Choose a GitHub installation.");
        project = await importProject.mutateAsync({
          installation_id: installationId,
          repository_id: source.repository.id,
          name: values.name.trim(),
          slug: values.slug,
          branch: values.branch.trim(),
        });
      } else {
        project = await create.mutateAsync({
          name: values.name.trim(),
          slug: values.slug,
          branch: values.branch.trim(),
          repository_url: values.repositoryUrl.trim(),
        });
      }
      await updateServiceConfig(project.id, {
        project_kind: values.projectKind,
        builder: values.builder,
        context_path: values.contextPath.trim(),
        dockerfile_path: "Dockerfile",
        static_output_dir:
          values.projectKind === "static"
            ? values.staticOutput || "dist"
            : null,
        exposed_port:
          values.projectKind === "service" ? Number(values.port) : null,
        healthcheck_path: null,
      });
      await client.invalidateQueries({ queryKey: ["projects"] });
      onClose();
    } catch (reason) {
      setMessage(errorMessage(reason));
    }
  }

  return (
    <div className="wizard-shell">
      <header>
        <div>
          <span className="eyebrow">CREATE PROJECT</span>
          <h2>{source ? "Configure deployment" : "Choose a source"}</h2>
        </div>
        <button
          className="icon-action"
          type="button"
          aria-label="Close project wizard"
          onClick={onClose}
        >
          ×
        </button>
      </header>
      {!source ? (
        <div className="wizard-source">
          <div className="source-toolbar">
            <div className="source-tabs">
              <button className="active" type="button">
                GitHub
              </button>
              <button
                type="button"
                onClick={() => {
                  setSource({ kind: "manual" });
                  setValues(initialValues);
                }}
              >
                Git URL
              </button>
            </div>
            {installUrl && (
              <a className="table-action" href={installUrl}>
                Install GitHub App
              </a>
            )}
          </div>
          {!configured && (
            <div className="github-app-setup">
              <div>
                <span className="eyebrow">ONE-TIME SETUP</span>
                <strong>Connect this Depot to GitHub</strong>
                <p>
                  Create a private GitHub App with the callback, webhook, and
                  read permissions already filled in. Depot encrypts the
                  generated credentials on this server.
                </p>
              </div>
              {setupAvailable && manifestUrl && manifest ? (
                <form
                  action={`${manifestUrl}?state=${encodeURIComponent(manifestState)}`}
                  method="post"
                  onSubmit={() =>
                    window.sessionStorage.setItem(
                      "depot_github_manifest_state",
                      manifestState,
                    )
                  }
                >
                  <input type="hidden" name="manifest" value={manifest} />
                  <button className="primary-action" type="submit">
                    Create GitHub App →
                  </button>
                </form>
              ) : (
                <span className="panel-message">
                  Ask a Depot administrator to complete GitHub setup.
                </span>
              )}
            </div>
          )}
          {integration.data?.installations.length ? (
            <div className="github-controls">
              <select
                aria-label="GitHub account"
                value={installationId ?? ""}
                onChange={(event) =>
                  setInstallationId(Number(event.target.value))
                }
              >
                {integration.data.installations.map((item) => (
                  <option
                    key={item.installation_id}
                    value={item.installation_id}
                  >
                    {item.account_login}
                  </option>
                ))}
              </select>
              <input
                aria-label="Search repositories"
                placeholder="Search repositories…"
                value={search}
                onChange={(event) => setSearch(event.target.value)}
              />
            </div>
          ) : (
            configured && (
              <div className="empty-state">
                <strong>No GitHub account connected.</strong>
                <span>Connect the GitHub App to browse repositories.</span>
              </div>
            )
          )}
          <div className="wizard-repositories">
            {repositories.isPending && installationId && (
              <p className="panel-message">Loading repositories…</p>
            )}
            {visible?.map((repository) => (
              <button
                type="button"
                key={repository.id}
                disabled={repository.archived}
                onClick={() => chooseRepository(repository)}
              >
                <span>
                  <strong>{repository.full_name}</strong>
                  <small>
                    {repository.private ? "Private" : "Public"} ·{" "}
                    {repository.default_branch}
                  </small>
                </span>
                <i aria-hidden="true">→</i>
              </button>
            ))}
          </div>
        </div>
      ) : (
        <div className="wizard-configure">
          <button
            className="project-back"
            type="button"
            onClick={() => setSource(undefined)}
          >
            ← Change source
          </button>
          <div className="wizard-form-grid">
            <label>
              Project name
              <input
                value={values.name}
                onChange={(event) =>
                  setValues({ ...values, name: event.target.value })
                }
              />
            </label>
            <label>
              Project slug
              <input
                value={values.slug}
                onChange={(event) =>
                  setValues({
                    ...values,
                    slug: projectSlug(event.target.value),
                  })
                }
              />
            </label>
            {source.kind === "manual" && (
              <label className="wide">
                Repository URL
                <input
                  type="url"
                  placeholder="https://github.com/you/project.git"
                  value={values.repositoryUrl}
                  onChange={(event) =>
                    setValues({ ...values, repositoryUrl: event.target.value })
                  }
                />
              </label>
            )}
            <label>
              Branch
              <input
                value={values.branch}
                onChange={(event) =>
                  setValues({ ...values, branch: event.target.value })
                }
              />
            </label>
            {source.kind === "manual" && (
              <label>
                Source directory
                <input
                  placeholder=". or apps/web"
                  value={values.contextPath}
                  onChange={(event) =>
                    setValues({ ...values, contextPath: event.target.value })
                  }
                />
              </label>
            )}
            {source.kind === "github" && installationId && (
              <div className="wide">
                <RepositoryDirectoryPicker
                  installationId={installationId}
                  repositoryId={source.repository.id}
                  reference={values.branch}
                  value={values.contextPath}
                  onChange={(contextPath) =>
                    setValues((current) => ({ ...current, contextPath }))
                  }
                />
              </div>
            )}
            <label>
              Project type
              <select
                value={values.projectKind}
                onChange={(event) =>
                  setValues({
                    ...values,
                    projectKind: event.target
                      .value as ConfigureValues["projectKind"],
                  })
                }
              >
                <option value="auto">Auto-detect</option>
                <option value="service">Web service</option>
                <option value="static">Static site</option>
              </select>
            </label>
            <label>
              Builder
              <select
                value={values.builder}
                onChange={(event) =>
                  setValues({
                    ...values,
                    builder: event.target.value as ConfigureValues["builder"],
                  })
                }
              >
                <option value="auto">Auto-detect</option>
                <option value="dockerfile">Dockerfile</option>
                <option value="railpack">Railpack</option>
              </select>
            </label>
            {values.projectKind === "static" && (
              <label>
                Output directory
                <input
                  value={values.staticOutput}
                  onChange={(event) =>
                    setValues({ ...values, staticOutput: event.target.value })
                  }
                />
              </label>
            )}
            {values.projectKind === "service" && (
              <label>
                Exposed port
                <input
                  type="number"
                  min="1"
                  max="65535"
                  placeholder="3000"
                  value={values.port}
                  onChange={(event) =>
                    setValues({ ...values, port: event.target.value })
                  }
                />
              </label>
            )}
          </div>
          <footer>
            <span>Settings remain editable after creation.</span>
            <button
              className="primary-action"
              type="button"
              disabled={create.isPending || importProject.isPending}
              onClick={() => void finish()}
            >
              {create.isPending || importProject.isPending
                ? "Creating…"
                : "Create project →"}
            </button>
          </footer>
        </div>
      )}
      {message && (
        <p className="notice danger" role="alert">
          {message}
        </p>
      )}
    </div>
  );
}

/** Converts an arbitrary repository name to Depot's stable slug format. */
function projectSlug(name: string) {
  const slug = name
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-|-$/g, "")
    .slice(0, 63);
  return slug.length >= 2 ? slug : `${slug || "app"}-project`;
}
/** Checks the backend-compatible project slug shape. */
function validSlug(slug: string) {
  return /^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/.test(slug);
}

/** Accepts a bounded repository-relative path without traversal segments. */
function validRepositoryPath(path: string) {
  const value = path.trim();
  return (
    value.length > 0 &&
    value.length <= 255 &&
    !value.startsWith("/") &&
    !value.startsWith("\\") &&
    ![...value].some((character) => {
      const code = character.charCodeAt(0);
      return code <= 31 || code === 127;
    }) &&
    !value.split(/[\\/]/).some((segment) => segment === "..")
  );
}

/** Generates the callback nonce that binds a GitHub manifest response to this browser tab. */
function createManifestState() {
  const bytes = window.crypto.getRandomValues(new Uint8Array(32));
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join(
    "",
  );
}

/** Maps supported template links to editable project defaults. */
function templateDefaults(
  template: string | null,
): ConfigureValues | undefined {
  if (template === "static") return { ...initialValues, projectKind: "static" };
  if (template === "service")
    return { ...initialValues, projectKind: "service" };
  return undefined;
}
/** Converts provider and validation failures into safe dialog feedback. */
function errorMessage(reason: unknown) {
  return reason instanceof ApiError || reason instanceof Error
    ? reason.message
    : "The project could not be created.";
}
