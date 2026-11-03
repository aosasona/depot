import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useMemo, useRef, useState } from "react";
import {
  ApiError,
  controlProjectRuntime,
  createProject,
  getLatestRuntimeAction,
  getServiceConfig,
  listDeployments,
  listDomains,
  listVariables,
  rollbackDeployment,
  type Project,
  updateServiceConfig,
  upsertVariable,
} from "../../lib/api/client";
import { ContainerFileBrowser } from "./ContainerFileBrowser";

type NodeKind = "app" | "database" | "redis" | "storage" | "domain";
type NodeAction =
  | "inspect"
  | "clone"
  | "shell"
  | "files"
  | "start"
  | "stop"
  | "restart"
  | "redeploy";
type CanvasNode = {
  id: string;
  kind: NodeKind;
  label: string;
  detail: string;
  variableName?: string;
  active?: boolean;
};
const resourcePatterns: Array<{ kind: NodeKind; pattern: RegExp }> = [
  { kind: "database", pattern: /DATABASE|POSTGRES|MYSQL|MONGO/ },
  { kind: "redis", pattern: /REDIS|VALKEY/ },
  { kind: "storage", pattern: /S3|STORAGE|BUCKET|R2_/ },
];

/** Maps one project's application, attached resources, and public routes. */
export function ProjectCanvas({ project }: { project: Project }) {
  const client = useQueryClient();
  const variables = useQuery({
    queryKey: ["projects", project.id, "variables"],
    queryFn: () => listVariables(project.id),
  });
  const domains = useQuery({
    queryKey: ["projects", project.id, "domains"],
    queryFn: () => listDomains(project.id),
  });
  const deployments = useQuery({
    queryKey: ["projects", project.id, "deployments"],
    queryFn: () => listDeployments(project.id),
  });
  const runtimeAction = useQuery({
    queryKey: ["projects", project.id, "runtime-action"],
    queryFn: () => getLatestRuntimeAction(project.id),
    refetchInterval: (query) =>
      query.state.data && ["queued", "running"].includes(query.state.data.state)
        ? 1_500
        : false,
  });
  const [selected, setSelected] = useState<CanvasNode>();
  const [menu, setMenu] = useState<{
    node: CanvasNode;
    x: number;
    y: number;
  }>();
  const [connection, setConnection] = useState<{
    kind: NodeKind;
    suggestedName?: string;
  }>();
  const [shellOpen, setShellOpen] = useState(false);
  const [filesOpen, setFilesOpen] = useState(false);
  const [message, setMessage] = useState<string>();
  const active = deployments.data?.find((deployment) => deployment.active);
  const control = useMutation({
    mutationFn: (action: "start" | "stop" | "restart") =>
      controlProjectRuntime(project.id, action),
    onSuccess: async () => {
      await client.invalidateQueries({
        queryKey: ["projects", project.id, "runtime-action"],
      });
    },
  });
  const redeploy = useMutation({
    mutationFn: (deploymentId: string) => rollbackDeployment(deploymentId),
    onSuccess: async () => {
      await client.invalidateQueries({
        queryKey: ["projects", project.id, "deployments"],
      });
    },
  });
  const serviceBusy =
    control.isPending ||
    redeploy.isPending ||
    Boolean(
      runtimeAction.data &&
        ["queued", "running"].includes(runtimeAction.data.state),
    );
  const nodes = useMemo(
    () =>
      buildNodes(
        project,
        variables.data ?? [],
        domains.data ?? [],
        active?.runtime_desired_state,
      ),
    [active, domains.data, project, variables.data],
  );

  useEffect(() => {
    /** Closes the node menu after the next document interaction. */
    function closeMenu() {
      setMenu(undefined);
    }
    document.addEventListener("click", closeMenu);
    return () => document.removeEventListener("click", closeMenu);
  }, []);

  useEffect(() => {
    if (
      runtimeAction.data &&
      !["queued", "running"].includes(runtimeAction.data.state)
    ) {
      void client.invalidateQueries({
        queryKey: ["projects", project.id, "deployments"],
      });
    }
  }, [client, project.id, runtimeAction.data]);

  useEffect(() => {
    const parameters = new URLSearchParams(window.location.search);
    const preset = resourceConnection(parameters.get("resource"));
    if (!preset) return;
    setConnection(preset);
    parameters.delete("resource");
    window.history.replaceState(
      {},
      "",
      `${window.location.pathname}${parameters.size ? `?${parameters}` : ""}`,
    );
  }, []);

  /** Opens the node menu at a pointer or keyboard-selected position. */
  function openMenu(event: React.MouseEvent, node: CanvasNode) {
    event.preventDefault();
    event.stopPropagation();
    setMenu({
      node,
      x: Math.min(event.clientX, window.innerWidth - 190),
      y: Math.min(event.clientY, window.innerHeight - 330),
    });
  }

  /** Clones project source and build configuration without copying secrets or domains. */
  async function cloneProject() {
    const name = window
      .prompt("Name for the cloned project", `${project.name} copy`)
      ?.trim();
    if (!name) return;
    const slug = `${slugify(name).slice(0, 50)}-${Date.now().toString(36).slice(-6)}`;
    setMessage("Cloning project configuration…");
    try {
      const config = await getServiceConfig(project.id);
      const created = await createProject({
        name,
        slug,
        repository_url: project.repository_url,
        branch: project.branch,
      });
      await updateServiceConfig(created.id, {
        project_kind: config.project_kind,
        builder: config.builder,
        context_path: config.context_path,
        dockerfile_path: config.dockerfile_path,
        static_output_dir: config.static_output_dir,
        exposed_port: config.exposed_port,
        healthcheck_path: config.healthcheck_path,
      });
      await client.invalidateQueries({ queryKey: ["projects"] });
      setMessage(
        `Created ${name}. Secrets and domains were intentionally not copied.`,
      );
    } catch (reason) {
      setMessage(errorMessage(reason));
    }
  }

  /** Dispatches a context action for the selected topology node. */
  function act(action: NodeAction, node: CanvasNode) {
    setMenu(undefined);
    if (action === "inspect") return setSelected(node);
    if (action === "shell" && node.kind === "app") return setShellOpen(true);
    if (action === "files" && node.kind === "app") return setFilesOpen(true);
    if (
      node.kind === "app" &&
      (action === "start" || action === "stop" || action === "restart")
    ) {
      const confirmed =
        action === "start" ||
        window.confirm(
          action === "stop"
            ? "Stop this service? Its domains will return unavailable until it is started again."
            : "Restart this service now? Active requests may be interrupted.",
        );
      if (confirmed) control.mutate(action);
      return;
    }
    if (action === "redeploy" && node.kind === "app" && active) {
      if (
        window.confirm(
          `Redeploy commit ${active.commit_sha.slice(0, 12)} for this service?`,
        )
      )
        redeploy.mutate(active.id);
      return;
    }
    if (action === "clone" && node.kind === "app") {
      void cloneProject();
      return;
    }
    if (action === "clone" && node.variableName)
      setConnection({
        kind: node.kind,
        suggestedName: `${node.variableName}_COPY`,
      });
  }

  return (
    <div className="project-canvas-layout">
      <div className="canvas-toolbar">
        <div>
          <strong>Project topology</strong>
          <span>Right-click any node for actions.</span>
        </div>
        <div className="canvas-toolbar-actions">
          <button
            className="table-action file-action"
            type="button"
            disabled={
              !active?.container_id ||
              active.runtime_desired_state === "stopped"
            }
            title={
              active?.container_id && active.runtime_desired_state !== "stopped"
                ? "Browse files in the active release container"
                : "Deploy the project before browsing files"
            }
            onClick={() => setFilesOpen(true)}
          >
            <span className="file-action-label" aria-hidden="true">
              []
            </span>{" "}
            Browse files
          </button>
          <button
            className="table-action shell-action"
            type="button"
            disabled={
              !active?.container_id ||
              active.runtime_desired_state === "stopped"
            }
            title={
              active?.container_id && active.runtime_desired_state !== "stopped"
                ? "Open a command shell in the active release container"
                : "Deploy the project before opening a shell"
            }
            onClick={() => setShellOpen(true)}
          >
            <span aria-hidden="true">&gt;_</span> Open shell
          </button>
          <button
            className="table-action"
            type="button"
            onClick={() => setConnection({ kind: "database" })}
          >
            + Add connection
          </button>
        </div>
      </div>
      <section
        className="project-topology"
        aria-label={`${project.name} topology`}
      >
        <div className="topology-app-column">
          <CanvasCard
            node={nodes[0]}
            onContextMenu={openMenu}
            onInspect={setSelected}
          />
        </div>
        <div className="topology-lines" aria-hidden="true" />
        <div className="topology-resource-column">
          {nodes.slice(1).map((node) => (
            <CanvasCard
              key={node.id}
              node={node}
              onContextMenu={openMenu}
              onInspect={setSelected}
            />
          ))}
          {nodes.length === 1 && (
            <button
              className="canvas-add-node"
              type="button"
              onClick={() => setConnection({ kind: "database" })}
            >
              + Connect database, Redis, or storage
            </button>
          )}
        </div>
      </section>
      {message && (
        <p className="notice success" role="status">
          {message}
        </p>
      )}
      {runtimeAction.data && (
        <p
          className={
            runtimeAction.data.state === "failed"
              ? "notice danger"
              : "notice success"
          }
          role="status"
        >
          Service operation {runtimeAction.data.state}.
          {runtimeAction.data.last_error && ` ${runtimeAction.data.last_error}`}
        </p>
      )}
      {(control.error || redeploy.error) && (
        <p className="notice danger" role="alert">
          {errorMessage(control.error ?? redeploy.error)}
        </p>
      )}
      {menu && (
        <NodeMenu
          menu={menu}
          onAction={act}
          service={{
            available: Boolean(active?.container_id),
            stopped: active?.runtime_desired_state === "stopped",
            busy: serviceBusy,
          }}
        />
      )}
      {selected && (
        <InspectDrawer
          node={selected}
          onClose={() => setSelected(undefined)}
          onShell={() => {
            setSelected(undefined);
            setShellOpen(true);
          }}
        />
      )}
      {connection && (
        <ConnectionDialog
          projectId={project.id}
          initial={connection}
          onClose={() => setConnection(undefined)}
          onSaved={async () => {
            setConnection(undefined);
            await client.invalidateQueries({
              queryKey: ["projects", project.id, "variables"],
            });
          }}
        />
      )}
      {shellOpen && (
        <ShellDialog
          projectId={project.id}
          available={Boolean(
            active?.container_id && active.runtime_desired_state !== "stopped",
          )}
          onClose={() => setShellOpen(false)}
        />
      )}
      {filesOpen && (
        <ContainerFileBrowser
          projectId={project.id}
          available={Boolean(
            active?.container_id && active.runtime_desired_state !== "stopped",
          )}
          onClose={() => setFilesOpen(false)}
        />
      )}
    </div>
  );
}

/** Builds resource nodes only from persisted variable metadata and domain state. */
function buildNodes(
  project: Project,
  variables: Awaited<ReturnType<typeof listVariables>>,
  domains: Awaited<ReturnType<typeof listDomains>>,
  runtimeState?: string,
): CanvasNode[] {
  const running = runtimeState === "running";
  const nodes: CanvasNode[] = [
    {
      id: project.id,
      kind: "app",
      label: project.name,
      detail:
        runtimeState === "stopped"
          ? "Stopped application"
          : running
            ? "Active application"
            : "Application not deployed",
      active: running,
    },
  ];
  for (const candidate of resourcePatterns) {
    const matches = variables.filter((variable) =>
      candidate.pattern.test(variable.name),
    );
    if (matches.length === 0) continue;
    const primary =
      matches.find((variable) => variable.name.endsWith("_URL")) ?? matches[0];
    nodes.push({
      id: `${candidate.kind}-${primary.id}`,
      kind: candidate.kind,
      label: resourceLabel(candidate.kind),
      detail: matches.map((variable) => variable.name).join(", "),
      variableName: primary.name,
      active: true,
    });
  }
  for (const domain of domains)
    nodes.push({
      id: domain.id,
      kind: "domain",
      label: domain.hostname,
      detail: `Public route · ${domain.routing_status}`,
      active: domain.routing_status === "active",
    });
  return nodes;
}

/** Renders one keyboard-accessible topology node. */
function CanvasCard({
  node,
  onContextMenu,
  onInspect,
}: {
  node: CanvasNode;
  onContextMenu: (event: React.MouseEvent, node: CanvasNode) => void;
  onInspect: (node: CanvasNode) => void;
}) {
  return (
    <button
      className={`topology-node ${node.kind} ${node.active ? "active" : ""}`}
      type="button"
      onClick={() => onInspect(node)}
      onContextMenu={(event) => onContextMenu(event, node)}
    >
      <span className="topology-icon" aria-hidden="true">
        {nodeIcon(node.kind)}
      </span>
      <span>
        <strong>{node.label}</strong>
        <small>{node.detail}</small>
      </span>
      <i aria-hidden="true" />
    </button>
  );
}

/** Presents node actions at the right-click position. */
function NodeMenu({
  menu,
  onAction,
  service,
}: {
  menu: { node: CanvasNode; x: number; y: number };
  onAction: (action: NodeAction, node: CanvasNode) => void;
  service: { available: boolean; stopped: boolean; busy: boolean };
}) {
  return (
    <div
      className="node-context-menu"
      style={{ left: menu.x, top: menu.y }}
      role="menu"
    >
      <strong>{menu.node.label}</strong>
      <button
        type="button"
        role="menuitem"
        onClick={() => onAction("inspect", menu.node)}
      >
        Inspect
      </button>
      {menu.node.kind !== "domain" && (
        <button
          type="button"
          role="menuitem"
          onClick={() => onAction("clone", menu.node)}
        >
          Clone
        </button>
      )}
      {menu.node.kind === "app" && (
        <>
          {service.available &&
            (service.stopped ? (
              <button
                type="button"
                role="menuitem"
                disabled={service.busy}
                onClick={() => onAction("start", menu.node)}
              >
                Start service
              </button>
            ) : (
              <>
                <button
                  type="button"
                  role="menuitem"
                  disabled={service.busy}
                  onClick={() => onAction("restart", menu.node)}
                >
                  Restart service
                </button>
                <button
                  type="button"
                  role="menuitem"
                  disabled={service.busy}
                  onClick={() => onAction("stop", menu.node)}
                >
                  Stop service
                </button>
              </>
            ))}
          {service.available && (
            <button
              type="button"
              role="menuitem"
              disabled={service.busy}
              onClick={() => onAction("redeploy", menu.node)}
            >
              Redeploy
            </button>
          )}
          <button
            type="button"
            role="menuitem"
            disabled={!service.available || service.stopped}
            onClick={() => onAction("files", menu.node)}
          >
            Browse files
          </button>
          <button
            type="button"
            role="menuitem"
            disabled={!service.available || service.stopped}
            onClick={() => onAction("shell", menu.node)}
          >
            Open shell
          </button>
        </>
      )}
    </div>
  );
}

/** Displays node metadata without exposing stored connection values. */
function InspectDrawer({
  node,
  onClose,
  onShell,
}: {
  node: CanvasNode;
  onClose: () => void;
  onShell: () => void;
}) {
  return (
    <aside className="canvas-inspector" aria-label={`${node.label} details`}>
      <header>
        <div>
          <span className="eyebrow">{node.kind}</span>
          <h3>{node.label}</h3>
        </div>
        <button
          className="icon-action"
          type="button"
          aria-label="Close inspector"
          onClick={onClose}
        >
          ×
        </button>
      </header>
      <dl>
        <div>
          <dt>Type</dt>
          <dd>{resourceLabel(node.kind)}</dd>
        </div>
        <div>
          <dt>Status</dt>
          <dd>{node.active ? "Connected" : "Not active"}</dd>
        </div>
        <div>
          <dt>Configuration</dt>
          <dd>{node.detail}</dd>
        </div>
        {node.variableName && (
          <div>
            <dt>Environment key</dt>
            <dd>
              <code>{node.variableName}</code>
            </dd>
          </div>
        )}
      </dl>
      {node.kind === "app" && (
        <button
          className="table-action canvas-inspector-action"
          type="button"
          disabled={!node.active}
          onClick={onShell}
        >
          {node.active ? ">_ Open container shell" : "Deploy to open a shell"}
        </button>
      )}
    </aside>
  );
}

/** Adds a write-only external resource URL represented by an environment key. */
function ConnectionDialog({
  projectId,
  initial,
  onClose,
  onSaved,
}: {
  projectId: string;
  initial: { kind: NodeKind; suggestedName?: string };
  onClose: () => void;
  onSaved: () => Promise<void>;
}) {
  const [kind, setKind] = useState<NodeKind>(initial.kind);
  const [name, setName] = useState(
    initial.suggestedName ?? defaultVariable(initial.kind),
  );
  const [value, setValue] = useState("");
  const [error, setError] = useState<string>();
  const save = useMutation({
    mutationFn: () =>
      upsertVariable(projectId, { name, value, is_secret: true }),
  });
  async function submit(event: React.FormEvent) {
    event.preventDefault();
    setError(undefined);
    try {
      await save.mutateAsync();
      await onSaved();
    } catch (reason) {
      setError(errorMessage(reason));
    }
  }
  return (
    <div className="canvas-dialog-backdrop">
      <form className="canvas-dialog" onSubmit={(event) => void submit(event)}>
        <header>
          <div>
            <span className="eyebrow">EXTERNAL RESOURCE</span>
            <h3>Add connection</h3>
          </div>
          <button
            className="icon-action"
            type="button"
            aria-label="Close"
            onClick={onClose}
          >
            ×
          </button>
        </header>
        <label>
          Resource type
          <select
            value={kind}
            onChange={(event) => {
              const next = event.target.value as NodeKind;
              setKind(next);
              setName(defaultVariable(next));
            }}
          >
            <option value="database">Database</option>
            <option value="redis">Redis / Valkey</option>
            <option value="storage">Object storage</option>
          </select>
        </label>
        <label>
          Environment key
          <input
            value={name}
            onChange={(event) => setName(event.target.value.toUpperCase())}
          />
        </label>
        <label>
          Connection URL / secret
          <input
            type="password"
            placeholder={connectionPlaceholder(name)}
            value={value}
            onChange={(event) => setValue(event.target.value)}
          />
        </label>
        <p>The value is encrypted and never displayed again.</p>
        {error && <p className="notice danger">{error}</p>}
        <footer>
          <button className="table-action" type="button" onClick={onClose}>
            Cancel
          </button>
          <button
            className="primary-action"
            type="submit"
            disabled={save.isPending || !value}
          >
            {save.isPending ? "Connecting…" : "Add connection"}
          </button>
        </footer>
      </form>
    </div>
  );
}

/** Streams a persistent active-container shell through the local worker broker. */
function ShellDialog({
  projectId,
  available,
  onClose,
}: {
  projectId: string;
  available: boolean;
  onClose: () => void;
}) {
  const [status, setStatus] = useState<
    "connecting" | "open" | "closed" | "error"
  >("connecting");
  const [output, setOutput] = useState("");
  const [command, setCommand] = useState("");
  const socket = useRef<WebSocket | undefined>(undefined);
  const terminal = useRef<HTMLPreElement>(null);
  useEffect(() => {
    if (!available) {
      setStatus("closed");
      return;
    }
    const protocol = window.location.protocol === "https:" ? "wss:" : "ws:";
    const connection = new WebSocket(
      `${protocol}//${window.location.host}/api/v1/projects/${projectId}/shell`,
    );
    connection.binaryType = "arraybuffer";
    socket.current = connection;
    const decoder = new TextDecoder();
    connection.onopen = () => setStatus("open");
    connection.onmessage = (event) => {
      const chunk =
        typeof event.data === "string"
          ? event.data
          : decoder.decode(event.data as ArrayBuffer, { stream: true });
      setOutput((current) => `${current}${chunk}`.slice(-200_000));
    };
    connection.onerror = () => setStatus("error");
    connection.onclose = () =>
      setStatus((current) => (current === "error" ? current : "closed"));
    return () => connection.close();
  }, [available, projectId]);
  useEffect(() => {
    if (terminal.current)
      terminal.current.scrollTop = terminal.current.scrollHeight;
  }, [output]);

  /** Sends one bounded command line while preserving shell state between commands. */
  function submit(event: React.FormEvent) {
    event.preventDefault();
    const value = command.trim();
    if (!value || socket.current?.readyState !== WebSocket.OPEN) return;
    socket.current.send(`${value}\n`);
    setOutput((current) => `${current}$ ${value}\n`);
    setCommand("");
  }
  return (
    <div className="canvas-dialog-backdrop">
      <div className="canvas-dialog shell-dialog browser-shell">
        <header>
          <div>
            <span className="eyebrow">ACTIVE CONTAINER</span>
            <h3>Browser shell</h3>
          </div>
          <span className={`shell-state ${status}`}>{status}</span>
          <button
            className="icon-action"
            type="button"
            aria-label="Close"
            onClick={onClose}
          >
            ×
          </button>
        </header>
        {available ? (
          <>
            <p>
              Commands run inside the active release. The session closes after
              15 minutes or when this dialog disconnects. Full-screen TTY
              programs are not supported yet.
            </p>
            <pre ref={terminal} className="browser-shell-output">
              {output || "Connecting to Depot worker…\n"}
            </pre>
            <form className="browser-shell-input" onSubmit={submit}>
              <span aria-hidden="true">$</span>
              <input
                aria-label="Shell command"
                autoComplete="off"
                spellCheck={false}
                value={command}
                disabled={status !== "open"}
                onChange={(event) => setCommand(event.target.value)}
              />
              <button
                type="submit"
                disabled={status !== "open" || !command.trim()}
              >
                Run
              </button>
            </form>
            <footer>
              <button
                className="table-action"
                type="button"
                onClick={() => setOutput("")}
              >
                Clear
              </button>
              <button className="table-action" type="button" onClick={onClose}>
                Disconnect
              </button>
            </footer>
          </>
        ) : (
          <p>This project has no active service container. Deploy it first.</p>
        )}
      </div>
    </div>
  );
}

/** Returns a stable label for each canvas resource type. */
function resourceLabel(kind: NodeKind) {
  return (
    {
      app: "Application",
      database: "Database",
      redis: "Redis / Valkey",
      storage: "Object storage",
      domain: "Public domain",
    } as const
  )[kind];
}
/** Returns a compact text glyph for one resource category. */
function nodeIcon(kind: NodeKind) {
  return (
    {
      app: "APP",
      database: "DB",
      redis: "R",
      storage: "S3",
      domain: "DNS",
    } as const
  )[kind];
}
/** Suggests the conventional secret name for a new resource connection. */
function defaultVariable(kind: NodeKind) {
  return (
    {
      database: "DATABASE_URL",
      redis: "REDIS_URL",
      storage: "S3_URL",
      app: "SERVICE_URL",
      domain: "DOMAIN",
    } as const
  )[kind];
}

/** Maps a catalog dependency template to the canvas's encrypted connection form. */
function resourceConnection(resource: string | null) {
  const presets: Record<string, { kind: NodeKind; suggestedName: string }> = {
    postgres: { kind: "database", suggestedName: "DATABASE_URL" },
    redis: { kind: "redis", suggestedName: "REDIS_URL" },
    mysql: { kind: "database", suggestedName: "MYSQL_URL" },
    s3: { kind: "storage", suggestedName: "S3_URL" },
  };
  return resource ? presets[resource] : undefined;
}
/** Shows the expected secret shape without supplying unsafe default credentials. */
function connectionPlaceholder(name: string) {
  const placeholders: Record<string, string> = {
    DATABASE_URL: "postgres://user:password@host:5432/database",
    REDIS_URL: "rediss://user:password@host:6379/0",
    MYSQL_URL: "mysql://user:password@host:3306/database",
    S3_URL: "https://access-key:secret@endpoint/bucket",
  };
  return placeholders[name] ?? "Service connection URL or secret";
}
/** Converts an entered project name into a safe slug prefix. */
function slugify(value: string) {
  return (
    value
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-|-$/g, "") || "project"
  );
}
/** Converts API failures into safe canvas feedback. */
function errorMessage(reason: unknown) {
  return reason instanceof ApiError || reason instanceof Error
    ? reason.message
    : "Depot could not complete the canvas action.";
}
