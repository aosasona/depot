import { useMutation } from "@tanstack/react-query";
import { useState } from "react";
import { z } from "zod";
import { ApiError, createGithubWebhook } from "../../lib/api/client";

const repositorySchema = z
  .string()
  .trim()
  .regex(
    /^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/,
    "Enter a GitHub repository as owner/name.",
  );

/** Creates a signed GitHub push endpoint and displays its credentials once. */
export function WebhookPanel({ projectId }: { projectId: string }) {
  const [repository, setRepository] = useState("");
  const [error, setError] = useState<string>();
  const webhook = useMutation({
    mutationFn: (repositoryFullName: string) =>
      createGithubWebhook(projectId, repositoryFullName),
    onMutate: () => setError(undefined),
    onError: (reason) =>
      setError(
        reason instanceof ApiError
          ? reason.message
          : "Depot could not create the webhook.",
      ),
  });

  const endpoint = webhook.data
    ? `${window.location.origin}${webhook.data.endpoint_path}`
    : undefined;
  return (
    <div className="domain-panel">
      <strong>GitHub push webhook</strong>
      <p className="config-message">
        Signed pushes to this project branch can enqueue exact-commit
        deployments.
      </p>
      <form
        className="domain-form"
        onSubmit={(event) => {
          event.preventDefault();
          const parsed = repositorySchema.safeParse(repository);
          if (!parsed.success) {
            setError(parsed.error.issues[0]?.message);
            return;
          }
          webhook.mutate(parsed.data);
        }}
      >
        <input
          aria-label="GitHub repository"
          autoComplete="off"
          placeholder="owner/repository"
          value={repository}
          onChange={(event) => setRepository(event.target.value)}
        />
        <button
          className="table-action domain-submit"
          type="submit"
          disabled={webhook.isPending}
        >
          {webhook.isPending ? "Creating…" : "Create / rotate"}
        </button>
      </form>
      {error && <p className="inspection-error">{error}</p>}
      {endpoint && webhook.data && (
        <div className="credential-result" role="status">
          <strong>Copy these values now. They will not be shown again.</strong>
          <span>Payload URL</span>
          <code>{endpoint}</code>
          <span>Secret</span>
          <code>{webhook.data.secret}</code>
          <span>Content type: application/json · Event: push only</span>
        </div>
      )}
    </div>
  );
}
