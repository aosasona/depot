import { queryOptions } from "@tanstack/react-query";
import { getBootstrapStatus, getCurrentUser } from "../../lib/api/client";

/** Defines cached setup state shared by authentication screens. */
export function bootstrapStatusQuery() {
  return queryOptions({
    queryKey: ["auth", "bootstrap"],
    queryFn: getBootstrapStatus,
  });
}

/** Defines session lookup without retrying expected unauthorized responses. */
export function currentUserQuery() {
  return queryOptions({
    queryKey: ["auth", "current-user"],
    queryFn: getCurrentUser,
    retry: false,
  });
}
