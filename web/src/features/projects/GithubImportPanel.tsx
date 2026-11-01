import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useMemo, useRef, useState } from "react";
import {
  type GithubRepository,
  getGithubIntegration,
  importGithubProject,
  linkGithubInstallation,
  listGithubRepositories,
} from "../../lib/api/client";

/** Connects a GitHub App installation and imports server-verified repositories. */
export function GithubImportPanel() {
  const queryClient = useQueryClient();
  const integration = useQuery({
    queryKey: ["github-integration"],
    queryFn: getGithubIntegration,
  });
  const [installationId, setInstallationId] = useState<number>();
  const [search, setSearch] = useState("");
  const [message, setMessage] = useState<string>();
  const linkedCallback = useRef(false);
  const link = useMutation({ mutationFn: linkGithubInstallation });

  useEffect(() => {
    const first = integration.data?.installations[0]?.installation_id;
    if (!installationId && first) setInstallationId(first);
  }, [installationId, integration.data]);

  useEffect(() => {
    if (!integration.data?.configured || linkedCallback.current) return;
    const params = new URLSearchParams(window.location.search);
    const rawId = params.get("installation_id");
    const callbackId = rawId ? Number(rawId) : 0;
    if (!Number.isSafeInteger(callbackId) || callbackId <= 0) return;
    linkedCallback.current = true;
    setMessage("Verifying GitHub installation…");
    void link.mutateAsync(callbackId).then(
      async () => {
        params.delete("installation_id");
        params.delete("setup_action");
        const query = params.toString();
        window.history.replaceState(
          {},
          "",
          `${window.location.pathname}${query ? `?${query}` : ""}`,
        );
        setInstallationId(callbackId);
        setMessage("GitHub installation connected.");
        await queryClient.invalidateQueries({
          queryKey: ["github-integration"],
        });
      },
      (reason: unknown) => setMessage(errorMessage(reason)),
    );
  }, [integration.data?.configured, link, queryClient]);

  const repositories = useQuery({
    queryKey: ["github-repositories", installationId],
    queryFn: () => listGithubRepositories(installationId ?? 0),
    enabled: Boolean(installationId),
  });
  const visible = useMemo(() => {
    const needle = search.trim().toLowerCase();
    return repositories.data?.filter((repository) =>
      repository.full_name.toLowerCase().includes(needle),
    );
  }, [repositories.data, search]);
  const importProject = useMutation({ mutationFn: importGithubProject });

  async function importRepository(repository: GithubRepository) {
    if (!installationId) return;
    setMessage(undefined);
    try {
      await importProject.mutateAsync({
        installation_id: installationId,
        repository_id: repository.id,
        slug: projectSlug(repository.name),
        name: repository.name,
      });
      setMessage(`${repository.full_name} was added to Depot.`);
      await queryClient.invalidateQueries({ queryKey: ["projects"] });
    } catch (reason) {
      setMessage(errorMessage(reason));
    }
  }

  if (integration.isPending)
    return <p className="panel-message">Loading GitHub…</p>;
  if (integration.isError)
    return <p className="notice danger">GitHub status is unavailable.</p>;
  if (!integration.data.configured) {
    return (
      <div className="github-import muted-integration">
        <strong>GitHub App is not configured</strong>
        <span>
          Add the GitHub App settings on the Depot host to browse repositories
          here.
        </span>
      </div>
    );
  }

  return (
    <div className="github-import">
      <div className="github-import-heading">
        <div>
          <strong>Import from GitHub</strong>
          <span>
            Choose a repository—Depot handles the connection and deploy hook.
          </span>
        </div>
        {integration.data.install_url && (
          <a
            className="table-action github-connect"
            href={integration.data.install_url}
          >
            Connect GitHub
          </a>
        )}
      </div>
      {integration.data.installations.length > 0 && (
        <div className="github-controls">
          <select
            aria-label="GitHub account"
            value={installationId ?? ""}
            onChange={(event) => setInstallationId(Number(event.target.value))}
          >
            {integration.data.installations.map((installation) => (
              <option
                key={installation.installation_id}
                value={installation.installation_id}
              >
                {installation.account_login}
              </option>
            ))}
          </select>
          <input
            aria-label="Search GitHub repositories"
            placeholder="Search repositories"
            value={search}
            onChange={(event) => setSearch(event.target.value)}
          />
        </div>
      )}
      {repositories.isPending && installationId && (
        <p className="panel-message">Loading repositories…</p>
      )}
      {repositories.isError && (
        <p className="notice danger">Repositories could not be loaded.</p>
      )}
      <div className="github-repositories">
        {visible?.map((repository) => (
          <div className="github-repository" key={repository.id}>
            <div>
              <strong>{repository.full_name}</strong>
              <span>
                {repository.private ? "Private" : "Public"} ·{" "}
                {repository.default_branch}
              </span>
            </div>
            <button
              className="table-action"
              type="button"
              disabled={repository.archived || importProject.isPending}
              onClick={() => void importRepository(repository)}
            >
              {repository.archived ? "Archived" : "Create project"}
            </button>
          </div>
        ))}
      </div>
      {message && (
        <p
          className={
            link.isError || importProject.isError
              ? "notice danger"
              : "notice success"
          }
        >
          {message}
        </p>
      )}
    </div>
  );
}

function projectSlug(name: string) {
  const slug = name
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-|-$/g, "")
    .slice(0, 63);
  return slug.length >= 2 ? slug : `${slug || "app"}-project`;
}

function errorMessage(reason: unknown) {
  return reason instanceof Error
    ? reason.message
    : "The GitHub request failed.";
}
