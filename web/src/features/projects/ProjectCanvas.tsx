import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useMemo, useState } from "react";
import {
  ApiError,
  createProject,
  getServiceConfig,
  listDeployments,
  listDomains,
  listVariables,
  type Project,
  updateServiceConfig,
  upsertVariable,
} from "../../lib/api/client";

type NodeKind = "app" | "database" | "redis" | "storage" | "domain";
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
  const [message, setMessage] = useState<string>();
  const active = deployments.data?.find((deployment) => deployment.active);
  const nodes = useMemo(
    () =>
      buildNodes(
        project,
        variables.data ?? [],
        domains.data ?? [],
        Boolean(active),
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

  /** Opens the node menu at a pointer or keyboard-selected position. */
  function openMenu(event: React.MouseEvent, node: CanvasNode) {
    event.preventDefault();
    event.stopPropagation();
    setMenu({
      node,
      x: Math.min(event.clientX, window.innerWidth - 190),
      y: Math.min(event.clientY, window.innerHeight - 180),
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
  function act(action: "inspect" | "clone" | "shell", node: CanvasNode) {
    setMenu(undefined);
    if (action === "inspect") return setSelected(node);
    if (action === "shell" && node.kind === "app") return setShellOpen(true);
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

  const shellCommand = active?.container_id
    ? `sudo -u depot-worker docker exec -it ${active.container_id} sh`
    : undefined;
  return (
    <div className="project-canvas-layout">
      <div className="canvas-toolbar">
        <div>
          <strong>Project topology</strong>
          <span>Right-click any node for actions.</span>
        </div>
        <button
          className="table-action"
          type="button"
          onClick={() => setConnection({ kind: "database" })}
        >
          + Add connection
        </button>
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
      {menu && <NodeMenu menu={menu} onAction={act} />}
      {selected && (
        <InspectDrawer node={selected} onClose={() => setSelected(undefined)} />
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
          command={shellCommand}
          onClose={() => setShellOpen(false)}
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
  active: boolean,
): CanvasNode[] {
  const nodes: CanvasNode[] = [
    {
      id: project.id,
      kind: "app",
      label: project.name,
      detail: active ? "Active application" : "Application not deployed",
      active,
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
}: {
  menu: { node: CanvasNode; x: number; y: number };
  onAction: (action: "inspect" | "clone" | "shell", node: CanvasNode) => void;
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
        <button
          type="button"
          role="menuitem"
          onClick={() => onAction("shell", menu.node)}
        >
          Open shell
        </button>
      )}
    </div>
  );
}

/** Displays node metadata without exposing stored connection values. */
function InspectDrawer({
  node,
  onClose,
}: {
  node: CanvasNode;
  onClose: () => void;
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

/** Shows the exact host command for entering the active container. */
function ShellDialog({
  command,
  onClose,
}: {
  command?: string;
  onClose: () => void;
}) {
  const [copied, setCopied] = useState(false);
  async function copy() {
    if (!command) return;
    await navigator.clipboard.writeText(command);
    setCopied(true);
  }
  return (
    <div className="canvas-dialog-backdrop">
      <div className="canvas-dialog shell-dialog">
        <header>
          <div>
            <span className="eyebrow">ACTIVE CONTAINER</span>
            <h3>Open a shell</h3>
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
        {command ? (
          <>
            <p>
              Run this on the Depot VM. Browser shells will follow after the
              worker control channel is isolated from the public API.
            </p>
            <pre>{command}</pre>
            <button
              className="primary-action"
              type="button"
              onClick={() => void copy()}
            >
              {copied ? "Copied" : "Copy command"}
            </button>
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
