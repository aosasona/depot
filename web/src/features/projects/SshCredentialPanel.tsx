import { useMutation } from "@tanstack/react-query";
import { useState } from "react";
import { ApiError, upsertSshCredential } from "../../lib/api/client";

/** Collects write-only SSH material for a private project repository. */
export function SshCredentialPanel({ projectId }: { projectId: string }) {
  const [privateKey, setPrivateKey] = useState("");
  const [knownHosts, setKnownHosts] = useState("");
  const [message, setMessage] = useState<string>();
  const credential = useMutation({
    mutationFn: () =>
      upsertSshCredential(projectId, {
        private_key: privateKey,
        known_hosts: knownHosts,
      }),
    onMutate: () => setMessage(undefined),
    onSuccess: () => {
      setPrivateKey("");
      setMessage("SSH deploy credential saved. The private key is now hidden.");
    },
    onError: (reason) =>
      setMessage(
        reason instanceof ApiError
          ? reason.message
          : "Depot could not save the SSH credential.",
      ),
  });

  return (
    <div className="domain-panel">
      <strong>Private repository SSH</strong>
      <p className="config-message">
        Paste a read-only deploy key and trusted known_hosts lines. Depot never
        returns the private key.
      </p>
      <form
        className="credential-form"
        onSubmit={(event) => {
          event.preventDefault();
          credential.mutate();
        }}
      >
        <label>
          Private key
          <textarea
            aria-label="SSH private key"
            autoComplete="off"
            placeholder="-----BEGIN OPENSSH PRIVATE KEY-----"
            required
            rows={5}
            value={privateKey}
            onChange={(event) => setPrivateKey(event.target.value)}
          />
        </label>
        <label>
          Trusted host keys
          <textarea
            aria-label="SSH known hosts"
            autoComplete="off"
            placeholder="github.com ssh-ed25519 AAAA…"
            required
            rows={4}
            value={knownHosts}
            onChange={(event) => setKnownHosts(event.target.value)}
          />
        </label>
        <button
          className="table-action credential-submit"
          type="submit"
          disabled={credential.isPending}
        >
          {credential.isPending ? "Encrypting…" : "Save / rotate credential"}
        </button>
      </form>
      {message && (
        <p
          className={credential.isError ? "inspection-error" : "notice success"}
          role="status"
        >
          {message}
        </p>
      )}
    </div>
  );
}
