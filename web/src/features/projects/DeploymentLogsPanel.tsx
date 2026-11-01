import { type UseQueryResult, useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import {
  Fragment,
  type CSSProperties,
  type ReactNode,
  useEffect,
  useRef,
  useState,
} from "react";
import {
  type Deployment,
  getJobStatus,
  type JobStatus,
  listDeployments,
  requestRuntimeLogs,
} from "../../lib/api/client";

type RuntimeLogResult = Extract<
  NonNullable<JobStatus["result"]>,
  { status: string }
>;

/** Shows durable deployment events and bounded builder output beside deployment work. */
export function DeploymentLogsPanel({
  projectId,
  deploymentId,
}: {
  projectId: string;
  deploymentId?: string;
}) {
  const [view, setView] = useState<"runtime" | "build">(
    deploymentId ? "build" : "runtime",
  );
  const deployments = useQuery({
    queryKey: ["projects", projectId, "deployments"],
    queryFn: () => listDeployments(projectId),
    refetchInterval: (query) =>
      query.state.data?.some((deployment) =>
        ["queued", "building"].includes(deployment.status),
      )
        ? 1_500
        : false,
  });
  return (
    <div className="deployment-logs">
      <div className="surface-heading">
        <div>
          <strong>{deploymentId ? "Deployment logs" : "Logs"}</strong>
          <span>
            {deploymentId
              ? "Durable events and output for this deployment attempt."
              : "Live service output and durable build history."}
          </span>
        </div>
        {deploymentId ? (
          <Link
            className="table-action"
            to="/projects/$projectId/$tab"
            params={{ projectId, tab: "deployments" }}
          >
            ← Deployment history
          </Link>
        ) : (
          <fieldset className="log-view-tabs">
            <legend>Log type</legend>
            <button
              className={view === "runtime" ? "active" : undefined}
              type="button"
              onClick={() => setView("runtime")}
            >
              Runtime
            </button>
            <button
              className={view === "build" ? "active" : undefined}
              type="button"
              onClick={() => setView("build")}
            >
              Build
            </button>
          </fieldset>
        )}
      </div>
      {view === "runtime" && <RuntimeLogs projectId={projectId} />}
      {view === "build" && (
        <BuildLogs deployments={deployments} deploymentId={deploymentId} />
      )}
    </div>
  );
}

/** Polls recent output for the active service while the runtime view is open. */
function RuntimeLogs({ projectId }: { projectId: string }) {
  const [lastRuntime, setLastRuntime] = useState<RuntimeLogResult>();
  const request = useQuery({
    queryKey: ["projects", projectId, "runtime-log-request"],
    queryFn: () => requestRuntimeLogs(projectId),
    refetchInterval: 5_000,
  });
  const job = useQuery({
    queryKey: ["jobs", request.data?.job_id],
    queryFn: () => getJobStatus(request.data?.job_id ?? ""),
    enabled: Boolean(request.data?.job_id),
    refetchInterval: (query) =>
      ["queued", "running"].includes(query.state.data?.state ?? "")
        ? 750
        : false,
  });
  const runtime =
    job.data?.result && "status" in job.data.result ? job.data.result : null;
  useEffect(() => {
    if (runtime) setLastRuntime(runtime);
  }, [runtime]);
  const visibleRuntime = runtime ?? lastRuntime;
  if ((request.isPending || job.isPending) && !visibleRuntime)
    return <p className="surface-empty">Connecting to the active service…</p>;
  if (
    (request.isError || job.isError || job.data?.state === "failed") &&
    !visibleRuntime
  )
    return (
      <p className="inspection-error">
        Runtime logs are unavailable. Check the Depot worker service.
      </p>
    );
  if (!visibleRuntime)
    return <p className="surface-empty">Collecting runtime output…</p>;
  if (visibleRuntime.status === "static")
    return (
      <p className="surface-empty">
        Static sites have no running process. Build output is available under
        Build.
      </p>
    );
  if (visibleRuntime.status === "not_deployed")
    return (
      <p className="surface-empty">
        Deploy this project to start collecting runtime output.
      </p>
    );
  return <RuntimeConsole runtime={visibleRuntime} />;
}

/** Tails the newest output unless the operator has deliberately scrolled upward. */
function RuntimeConsole({ runtime }: { runtime: RuntimeLogResult }) {
  const output = useRef<HTMLPreElement>(null);
  const following = useRef(true);
  useEffect(() => {
    const consoleOutput = output.current;
    if (!consoleOutput || !following.current) return;
    consoleOutput.scrollTop = consoleOutput.scrollHeight;
  }, [runtime.logs]);
  return (
    <div className="runtime-console">
      <div>
        <span>ACTIVE RELEASE</span>
        <code>{runtime.deployment_id?.slice(0, 12)}</code>
        <time>
          Updated {new Date(runtime.fetched_at * 1_000).toLocaleString()}
        </time>
      </div>
      <pre
        ref={output}
        onScroll={(event) => {
          const consoleOutput = event.currentTarget;
          following.current =
            consoleOutput.scrollHeight -
              consoleOutput.scrollTop -
              consoleOutput.clientHeight <
            48;
        }}
      >
        <ConsoleText
          value={runtime.logs || "The service has not written any output yet."}
        />
      </pre>
    </div>
  );
}

/** Renders retained worker events and build output for all deployment attempts. */
function BuildLogs({
  deployments,
  deploymentId,
}: {
  deployments: UseQueryResult<Deployment[], Error>;
  deploymentId?: string;
}) {
  const visibleDeployments = deploymentId
    ? deployments.data?.filter((deployment) => deployment.id === deploymentId)
    : deployments.data;
  return (
    <>
      {deployments.isPending && <p className="surface-empty">Loading logs…</p>}
      {deployments.isError && (
        <p className="inspection-error">Deployment logs are unavailable.</p>
      )}
      {!deploymentId && deployments.data?.length === 0 && (
        <p className="surface-empty">No deployment jobs have run yet.</p>
      )}
      {deploymentId &&
        deployments.isSuccess &&
        visibleDeployments?.length === 0 && (
          <p className="inspection-error">
            This deployment does not exist in this project.
          </p>
        )}
      <div className="deployment-log-list">
        {visibleDeployments?.map((deployment, index) => (
          <DeploymentLog
            deploymentId={deployment.id}
            jobId={deployment.job_id}
            commitSha={deployment.commit_sha}
            status={deployment.status}
            defaultOpen={index === 0}
            key={deployment.id}
          />
        ))}
      </div>
    </>
  );
}

function DeploymentLog({
  deploymentId,
  jobId,
  commitSha,
  status,
  defaultOpen,
}: {
  deploymentId: string;
  jobId: string;
  commitSha: string;
  status: string;
  defaultOpen: boolean;
}) {
  const job = useQuery({
    queryKey: ["jobs", jobId],
    queryFn: () => getJobStatus(jobId),
    refetchInterval: (query) =>
      ["queued", "running"].includes(query.state.data?.state ?? "")
        ? 1_500
        : false,
  });
  const result = job.data?.result;
  const output = result && "logs" in result ? result.logs : undefined;
  return (
    <details className="deployment-log" open={defaultOpen}>
      <summary>
        <span className={`deployment-status ${status}`}>{status}</span>
        <code>{commitSha.slice(0, 12)}</code>
        <span>{deploymentId.slice(0, 8)}</span>
        <span aria-hidden="true">⌄</span>
      </summary>
      <div className="log-stream">
        {job.isPending && <span>Loading job events…</span>}
        {job.data?.events.map((event) => (
          <div className={`log-line ${event.level}`} key={event.sequence}>
            <time>{formatTime(event.created_at)}</time>
            <span>{event.level}</span>
            <p>{event.message}</p>
          </div>
        ))}
        {output && (
          <pre>
            <ConsoleText value={output} />
          </pre>
        )}
        {job.data?.last_error && (
          <pre className="log-error">
            <ConsoleText value={job.data.last_error} />
          </pre>
        )}
        {!job.isPending && job.data?.events.length === 0 && !output && (
          <span>No output was retained for this job.</span>
        )}
      </div>
    </details>
  );
}

const ANSI_COLORS = [
  "#18181b",
  "#f87171",
  "#4ade80",
  "#facc15",
  "#60a5fa",
  "#d4d4d8",
  "#22d3ee",
  "#e4e4e7",
  "#71717a",
  "#fca5a5",
  "#86efac",
  "#fde047",
  "#93c5fd",
  "#fafafa",
  "#67e8f9",
  "#ffffff",
] as const;
const ESCAPE = String.fromCharCode(27);
const BELL = String.fromCharCode(7);

/** Renders terminal SGR colours as safe React spans and highlights plain structured logs. */
function ConsoleText({ value }: { value: string }) {
  const lines = value.split("\n");
  return lines.map((line, index) => (
    <Fragment key={`${index}-${line.slice(0, 24)}`}>
      <ConsoleLine value={line} />
      {index < lines.length - 1 && "\n"}
    </Fragment>
  ));
}

/** Chooses terminal rendering when escape codes exist, with a plain-log fallback. */
function ConsoleLine({ value }: { value: string }) {
  if (value.includes(`${ESCAPE}[`)) return renderAnsiLine(value);
  const tokens = value.split(
    /(\d{4}-\d{2}-\d{2}T\S+Z|\b(?:TRACE|DEBUG|INFO|WARN|WARNING|ERROR|FATAL)\b|\b[1-5]\d{2}\b|\b(?:GET|POST|PUT|PATCH|DELETE|HEAD|OPTIONS)\b)/g,
  );
  return tokens.map((token, index) => {
    const kind = plainLogTokenKind(token);
    return kind ? (
      <span className={`console-${kind}`} key={`${index}-${token}`}>
        {token}
      </span>
    ) : (
      token
    );
  });
}

/** Maps common structured-log fields to restrained terminal colours. */
function plainLogTokenKind(token: string) {
  if (/^\d{4}-\d{2}-\d{2}T\S+Z$/.test(token)) return "timestamp";
  if (/^(ERROR|FATAL)$/.test(token) || /^5\d\d$/.test(token)) return "error";
  if (/^(WARN|WARNING)$/.test(token) || /^4\d\d$/.test(token)) return "warning";
  if (/^(INFO|GET|POST|PUT|PATCH|DELETE|HEAD|OPTIONS)$/.test(token))
    return "info";
  if (/^(TRACE|DEBUG)$/.test(token)) return "debug";
  if (/^[1-3]\d\d$/.test(token)) return "success";
}

/** Interprets bounded ANSI SGR sequences without inserting application-owned HTML. */
function renderAnsiLine(value: string): ReactNode[] {
  const output: ReactNode[] = [];
  const style: CSSProperties = {};
  const expression = new RegExp(`${ESCAPE}\\[([0-9;]*)m`, "g");
  let start = 0;
  let match = expression.exec(value);
  while (match) {
    pushAnsiText(output, value.slice(start, match.index), style);
    applySgr(style, match[1]);
    start = expression.lastIndex;
    match = expression.exec(value);
  }
  pushAnsiText(output, value.slice(start), style);
  return output;
}

/** Appends one styled terminal segment while discarding unsupported control sequences. */
function pushAnsiText(output: ReactNode[], text: string, style: CSSProperties) {
  const operatingSystemCommand = new RegExp(
    `${ESCAPE}\\][^${BELL}]*(?:${BELL}|${ESCAPE}\\\\)`,
    "g",
  );
  const controlSequence = new RegExp(`${ESCAPE}\\[[0-?]*[ -/]*[@-~]`, "g");
  const clean = text
    .replace(operatingSystemCommand, "")
    .replace(controlSequence, "");
  if (!clean) return;
  output.push(
    Object.keys(style).length ? (
      <span style={{ ...style }} key={output.length}>
        {clean}
      </span>
    ) : (
      clean
    ),
  );
}

/** Applies the common foreground, background, weight, and decoration SGR codes. */
function applySgr(style: CSSProperties, encoded: string) {
  const codes = (encoded || "0").split(";").map(Number);
  for (let index = 0; index < codes.length; index += 1) {
    const code = codes[index];
    if (code === 0) {
      for (const property of Object.keys(style) as (keyof CSSProperties)[])
        delete style[property];
    } else if (code === 1) style.fontWeight = 700;
    else if (code === 2) style.opacity = 0.65;
    else if (code === 3) style.fontStyle = "italic";
    else if (code === 4) style.textDecoration = "underline";
    else if (code === 22) {
      delete style.fontWeight;
      delete style.opacity;
    } else if (code === 23) delete style.fontStyle;
    else if (code === 24) delete style.textDecoration;
    else if (code === 39) delete style.color;
    else if (code === 49) delete style.backgroundColor;
    else if (code >= 30 && code <= 37) style.color = ANSI_COLORS[code - 30];
    else if (code >= 90 && code <= 97) style.color = ANSI_COLORS[code - 82];
    else if (code >= 40 && code <= 47)
      style.backgroundColor = ANSI_COLORS[code - 40];
    else if (code >= 100 && code <= 107)
      style.backgroundColor = ANSI_COLORS[code - 92];
    else if (code === 38 || code === 48) {
      const property = code === 38 ? "color" : "backgroundColor";
      const mode = codes[index + 1];
      if (mode === 5 && codes[index + 2] !== undefined) {
        style[property] = ansi256Color(codes[index + 2]);
        index += 2;
      } else if (mode === 2 && codes.slice(index + 2, index + 5).length === 3) {
        const [red, green, blue] = codes
          .slice(index + 2, index + 5)
          .map((part) => Math.max(0, Math.min(255, part)));
        style[property] = `rgb(${red} ${green} ${blue})`;
        index += 4;
      }
    }
  }
}

/** Converts an xterm 256-colour index to a browser colour. */
function ansi256Color(value: number) {
  const index = Math.max(0, Math.min(255, value));
  if (index < 16) return ANSI_COLORS[index];
  if (index >= 232) {
    const channel = 8 + (index - 232) * 10;
    return `rgb(${channel} ${channel} ${channel})`;
  }
  const cube = index - 16;
  const levels = [0, 95, 135, 175, 215, 255];
  const red = levels[Math.floor(cube / 36)];
  const green = levels[Math.floor((cube % 36) / 6)];
  const blue = levels[cube % 6];
  return `rgb(${red} ${green} ${blue})`;
}

function formatTime(timestamp: number) {
  return new Date(timestamp * 1_000).toLocaleString([], {
    year: "numeric",
    month: "short",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  });
}
