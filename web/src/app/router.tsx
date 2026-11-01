import {
  createRootRoute,
  createRoute,
  createRouter,
  Outlet,
  redirect,
} from "@tanstack/react-router";
import {
  AuthEntryPage,
  type DashboardLocation,
} from "../features/auth/AuthEntryPage";
import type { ProjectTab } from "../features/projects/ProjectsPanel";

const validProjectTabs = new Set<ProjectTab>([
  "overview",
  "canvas",
  "deployments",
  "logs",
  "environment",
  "domains",
  "settings",
]);

/** Provides the route outlet shared by Depot's typed route tree. */
function RootLayout() {
  return <Outlet />;
}

function DashboardPage({ location }: { location: DashboardLocation }) {
  return <AuthEntryPage dashboard={location} />;
}

const rootRoute = createRootRoute({ component: RootLayout });
const indexRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/",
  // Keep the root as a projects alias because GitHub App setup callbacks append
  // installation parameters to the configured public origin.
  component: () => <DashboardPage location={{ kind: "projects" }} />,
});

const projectsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/projects",
  component: () => <DashboardPage location={{ kind: "projects" }} />,
});

const projectOverviewRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/projects/$projectId",
  beforeLoad: ({ params }) => {
    throw redirect({
      to: "/projects/$projectId/$tab",
      params: { projectId: params.projectId, tab: "overview" },
      replace: true,
    });
  },
});

const projectRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/projects/$projectId/$tab",
  beforeLoad: ({ params }) => {
    if (!validProjectTabs.has(params.tab as ProjectTab)) {
      throw redirect({
        to: "/projects/$projectId/$tab",
        params: { projectId: params.projectId, tab: "overview" },
        replace: true,
      });
    }
  },
  component: ProjectRoute,
});

function ProjectRoute() {
  const { projectId, tab } = projectRoute.useParams();
  return (
    <DashboardPage
      location={{ kind: "project", projectId, tab: tab as ProjectTab }}
    />
  );
}

const teamRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/team",
  component: () => <DashboardPage location={{ kind: "team" }} />,
});

const templatesRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/templates",
  component: () => <DashboardPage location={{ kind: "templates" }} />,
});

const routeTree = rootRoute.addChildren([
  indexRoute,
  projectsRoute,
  projectOverviewRoute,
  projectRoute,
  templatesRoute,
  teamRoute,
]);

export const router = createRouter({ routeTree });

declare module "@tanstack/react-router" {
  interface Register {
    router: typeof router;
  }
}
