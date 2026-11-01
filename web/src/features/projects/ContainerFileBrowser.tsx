import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useRef, useState } from "react";
import {
  ApiError,
  createContainerDirectory,
  deleteContainerFile,
  downloadContainerFile,
  listContainerFiles,
  renameContainerFile,
  uploadContainerFile,
} from "../../lib/api/client";

type ContainerFileBrowserProps = {
  projectId: string;
  available: boolean;
  onClose: () => void;
};

/** Browses and mutates the active container through Depot's bounded worker API. */
export function ContainerFileBrowser({
  projectId,
  available,
  onClose,
}: ContainerFileBrowserProps) {
  const client = useQueryClient();
  const [path, setPath] = useState("/");
  const [busy, setBusy] = useState(false);
  const [dragging, setDragging] = useState(false);
  const [message, setMessage] = useState<string>();
  const picker = useRef<HTMLInputElement>(null);
  const files = useQuery({
    queryKey: ["projects", projectId, "container-files", path],
    queryFn: () => listContainerFiles(projectId, path),
    enabled: available,
  });

  /** Refreshes the currently visible directory after a successful mutation. */
  async function refresh() {
    await client.invalidateQueries({
      queryKey: ["projects", projectId, "container-files", path],
    });
  }

  /** Uploads selected browser files into the open directory one at a time. */
  async function upload(filesToUpload: FileList | File[]) {
    const selected = Array.from(filesToUpload);
    if (selected.length === 0) return;
    setBusy(true);
    setMessage(
      `Uploading ${selected.length} file${selected.length === 1 ? "" : "s"}…`,
    );
    try {
      let uploaded = 0;
      for (const file of selected) {
        if (!validName(file.name))
          throw new Error(`Unsupported file name: ${file.name}`);
        const exists = files.data?.some((entry) => entry.name === file.name);
        if (exists && !window.confirm(`Replace ${joinPath(path, file.name)}?`))
          continue;
        await uploadContainerFile(projectId, joinPath(path, file.name), file);
        uploaded += 1;
      }
      setMessage(
        uploaded === 0
          ? "No files were changed."
          : `Uploaded ${uploaded} file${uploaded === 1 ? "" : "s"}.`,
      );
      await refresh();
    } catch (error) {
      setMessage(fileError(error));
    } finally {
      setBusy(false);
      if (picker.current) picker.current.value = "";
    }
  }

  /** Handles files dropped from the user's desktop onto the upload target. */
  function drop(event: React.DragEvent) {
    event.preventDefault();
    setDragging(false);
    void upload(event.dataTransfer.files);
  }

  /** Creates a named child directory under the current path. */
  async function createDirectory() {
    const name = window.prompt("New directory name")?.trim();
    if (!name || !validName(name)) {
      if (name)
        setMessage(
          "Directory names cannot contain slashes or control characters.",
        );
      return;
    }
    setBusy(true);
    try {
      await createContainerDirectory(projectId, joinPath(path, name));
      setMessage(`Created ${name}.`);
      await refresh();
    } catch (error) {
      setMessage(fileError(error));
    } finally {
      setBusy(false);
    }
  }

  /** Downloads a selected regular file using a temporary object URL. */
  async function download(name: string) {
    setBusy(true);
    try {
      const blob = await downloadContainerFile(projectId, joinPath(path, name));
      const url = URL.createObjectURL(blob);
      const anchor = document.createElement("a");
      anchor.href = url;
      anchor.download = name;
      anchor.click();
      URL.revokeObjectURL(url);
      setMessage(`Downloaded ${name}.`);
    } catch (error) {
      setMessage(fileError(error));
    } finally {
      setBusy(false);
    }
  }

  /** Renames a selected child while keeping it in the current directory. */
  async function rename(name: string) {
    const nextName = window.prompt("Rename entry", name)?.trim();
    if (!nextName || nextName === name) return;
    if (!validName(nextName)) {
      setMessage("Names cannot contain slashes or control characters.");
      return;
    }
    const exists = files.data?.some((entry) => entry.name === nextName);
    if (exists && !window.confirm(`Replace ${joinPath(path, nextName)}?`))
      return;
    setBusy(true);
    try {
      await renameContainerFile(
        projectId,
        joinPath(path, name),
        joinPath(path, nextName),
      );
      setMessage(`Renamed ${name} to ${nextName}.`);
      await refresh();
    } catch (error) {
      setMessage(fileError(error));
    } finally {
      setBusy(false);
    }
  }

  /** Deletes a selected child only after a destructive-action confirmation. */
  async function remove(name: string) {
    if (!window.confirm(`Delete ${joinPath(path, name)} recursively?`)) return;
    setBusy(true);
    try {
      await deleteContainerFile(projectId, joinPath(path, name));
      setMessage(`Deleted ${name}.`);
      await refresh();
    } catch (error) {
      setMessage(fileError(error));
    } finally {
      setBusy(false);
    }
  }

  const breadcrumbs = path === "/" ? [] : path.slice(1).split("/");
  return (
    <div className="canvas-dialog-backdrop">
      <div className="canvas-dialog container-file-browser">
        <header>
          <div>
            <span className="eyebrow">ACTIVE CONTAINER</span>
            <h3>Files</h3>
          </div>
          <span className="file-browser-limit">32 MiB / file</span>
          <button
            className="icon-action"
            type="button"
            aria-label="Close"
            onClick={onClose}
          >
            ×
          </button>
        </header>
        {!available ? (
          <p>This project has no active service container. Deploy it first.</p>
        ) : (
          <>
            <nav className="file-breadcrumbs" aria-label="Container path">
              <button type="button" onClick={() => setPath("/")}>
                /
              </button>
              {breadcrumbs.map((part, index) => {
                const target = `/${breadcrumbs.slice(0, index + 1).join("/")}`;
                return (
                  <span key={target}>
                    <i>/</i>
                    <button type="button" onClick={() => setPath(target)}>
                      {part}
                    </button>
                  </span>
                );
              })}
            </nav>
            <div className="file-browser-actions">
              <button
                className="table-action"
                type="button"
                disabled={busy}
                onClick={createDirectory}
              >
                New folder
              </button>
              <button
                className="table-action primary"
                type="button"
                disabled={busy}
                onClick={() => picker.current?.click()}
              >
                Upload files
              </button>
              <input
                ref={picker}
                type="file"
                multiple
                hidden
                onChange={(event) =>
                  event.target.files && void upload(event.target.files)
                }
              />
            </div>
            <button
              className={`file-drop-zone${dragging ? " dragging" : ""}`}
              type="button"
              disabled={busy}
              onClick={() => picker.current?.click()}
              onDragEnter={(event) => {
                event.preventDefault();
                setDragging(true);
              }}
              onDragOver={(event) => event.preventDefault()}
              onDragLeave={() => setDragging(false)}
              onDrop={drop}
            >
              <strong>Drop files here</strong>
              <span>or choose files from your PC</span>
            </button>
            <div className="file-list" aria-busy={files.isLoading || busy}>
              {files.isLoading && <p>Reading {path}…</p>}
              {files.error && (
                <p className="error-text">{fileError(files.error)}</p>
              )}
              {files.data?.length === 0 && <p>This directory is empty.</p>}
              {files.data?.map((entry) => (
                <div className="file-row" key={entry.name}>
                  <button
                    className="file-entry"
                    type="button"
                    disabled={busy || entry.kind === "symlink"}
                    onClick={() =>
                      entry.kind === "directory"
                        ? setPath(joinPath(path, entry.name))
                        : void download(entry.name)
                    }
                  >
                    <span aria-hidden="true">
                      {entry.kind === "directory"
                        ? "DIR"
                        : entry.kind === "symlink"
                          ? "LNK"
                          : "FILE"}
                    </span>
                    <strong>{entry.name}</strong>
                    <small>
                      {entry.kind === "file"
                        ? formatBytes(entry.size)
                        : entry.kind}
                    </small>
                  </button>
                  <button
                    type="button"
                    disabled={busy}
                    onClick={() => void rename(entry.name)}
                  >
                    Rename
                  </button>
                  <button
                    className="danger-action"
                    type="button"
                    disabled={busy}
                    onClick={() => void remove(entry.name)}
                  >
                    Delete
                  </button>
                </div>
              ))}
            </div>
            {message && <p className="file-browser-message">{message}</p>}
            <footer>
              <span>
                Changes are lost when this container is replaced unless backed
                by persistent storage.
              </span>
              <button className="table-action" type="button" onClick={onClose}>
                Close
              </button>
            </footer>
          </>
        )}
      </div>
    </div>
  );
}

/** Joins one validated child name to a canonical absolute container path. */
function joinPath(parent: string, name: string) {
  return parent === "/" ? `/${name}` : `${parent}/${name}`;
}

/** Rejects names which cannot be represented by Depot's path API. */
function validName(name: string) {
  return (
    name.length > 0 &&
    name !== "." &&
    name !== ".." &&
    !name.includes("/") &&
    !Array.from(name).some((character) => (character.codePointAt(0) ?? 0) < 32)
  );
}

/** Formats one byte count for compact file-list metadata. */
function formatBytes(bytes: number) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KiB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MiB`;
}

/** Converts unknown API failures into one useful browser message. */
function fileError(error: unknown) {
  return error instanceof ApiError || error instanceof Error
    ? error.message
    : "The container file operation failed.";
}
