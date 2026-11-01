import { useQuery } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { ApiError, listGithubRepositoryDirectory } from "../../lib/api/client";

type RepositoryDirectoryPickerProps = {
  installationId: number;
  repositoryId: number;
  reference: string;
  value: string;
  onChange: (path: string) => void;
};

/** Lets a GitHub project choose a repository directory as its build context. */
export function RepositoryDirectoryPicker({
  installationId,
  repositoryId,
  reference,
  value,
  onChange,
}: RepositoryDirectoryPickerProps) {
  const [path, setPath] = useState(value === "." ? "" : value);
  useEffect(() => setPath(value === "." ? "" : value), [value]);
  const entries = useQuery({
    queryKey: [
      "github-repository-directory",
      installationId,
      repositoryId,
      reference,
      path,
    ],
    queryFn: () =>
      listGithubRepositoryDirectory(
        installationId,
        repositoryId,
        reference,
        path,
      ),
    enabled: Boolean(reference.trim()),
  });
  const segments = path ? path.split("/") : [];
  return (
    <section className="repository-browser" aria-label="Repository files">
      <header>
        <div>
          <span className="eyebrow">SOURCE DIRECTORY</span>
          <strong>Choose the app folder</strong>
        </div>
        <button
          className="table-action"
          type="button"
          onClick={() => onChange(path || ".")}
        >
          Use this folder
        </button>
      </header>
      <nav aria-label="Current repository directory">
        <button type="button" onClick={() => setPath("")}>
          repository
        </button>
        {segments.map((segment, index) => {
          const target = segments.slice(0, index + 1).join("/");
          return (
            <span key={target}>
              <i aria-hidden="true">/</i>
              <button type="button" onClick={() => setPath(target)}>
                {segment}
              </button>
            </span>
          );
        })}
      </nav>
      <div className="repository-browser-list">
        {entries.isPending && (
          <span className="panel-message">Loading repository…</span>
        )}
        {entries.isError && (
          <span className="notice danger">
            {repositoryErrorMessage(entries.error)}
          </span>
        )}
        {entries.data?.map((entry) =>
          entry.kind === "dir" ? (
            <button
              type="button"
              key={entry.path}
              onClick={() => setPath(entry.path)}
            >
              <span className="repository-entry-icon" aria-hidden="true">
                ▸
              </span>
              <span>{entry.name}</span>
              <small>Folder</small>
            </button>
          ) : (
            <div className="repository-file" key={entry.path}>
              <span className="repository-entry-icon" aria-hidden="true">
                ·
              </span>
              <span>{entry.name}</span>
              <small>File</small>
            </div>
          ),
        )}
        {entries.data?.length === 0 && (
          <span className="panel-message">This folder is empty.</span>
        )}
      </div>
      <footer>
        <span>Selected build context</span>
        <code>{value}</code>
      </footer>
    </section>
  );
}

/** Keeps safe API diagnostics while retaining a useful fallback for network failures. */
function repositoryErrorMessage(reason: unknown) {
  return reason instanceof ApiError || reason instanceof Error
    ? reason.message
    : "This directory could not be loaded from GitHub.";
}
