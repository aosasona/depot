import { useQuery } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { ApiError, listGithubRepositoryDirectory } from "../../lib/api/client";

type RepositoryDirectoryPickerProps = {
  installationId: number;
  repositoryId: number;
  reference: string;
  value: string;
  selectionMode?: "directory" | "file";
  title?: string;
  onChange: (path: string) => void;
};

/** Lets a GitHub project choose a repository directory as its build context. */
export function RepositoryDirectoryPicker({
  installationId,
  repositoryId,
  reference,
  value,
  selectionMode = "directory",
  title,
  onChange,
}: RepositoryDirectoryPickerProps) {
  const [path, setPath] = useState(browserStartPath(value, selectionMode));
  useEffect(
    () => setPath(browserStartPath(value, selectionMode)),
    [selectionMode, value],
  );
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
          <span className="eyebrow">REPOSITORY BROWSER</span>
          <strong>
            {title ??
              (selectionMode === "file" ? "Choose a file" : "Choose a folder")}
          </strong>
        </div>
        {selectionMode === "directory" && (
          <button
            className="table-action"
            type="button"
            onClick={() => onChange(path || ".")}
          >
            Use this folder
          </button>
        )}
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
          ) : selectionMode === "file" ? (
            <button
              className="repository-file"
              type="button"
              key={entry.path}
              onClick={() => onChange(entry.path)}
            >
              <span className="repository-entry-icon" aria-hidden="true">
                ·
              </span>
              <span>{entry.name}</span>
              <small>Select file</small>
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
        <span>Selected {selectionMode === "file" ? "file" : "directory"}</span>
        <code>{value}</code>
      </footer>
    </section>
  );
}

/** Opens file selections at their parent directory and directory selections at themselves. */
function browserStartPath(value: string, selectionMode: "directory" | "file") {
  if (value === "." || !value) return "";
  if (selectionMode === "directory") return value;
  const segments = value.split("/");
  segments.pop();
  return segments.join("/");
}

/** Keeps safe API diagnostics while retaining a useful fallback for network failures. */
function repositoryErrorMessage(reason: unknown) {
  return reason instanceof ApiError || reason instanceof Error
    ? reason.message
    : "This directory could not be loaded from GitHub.";
}
