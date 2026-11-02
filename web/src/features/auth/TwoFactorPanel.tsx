import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import {
  ApiError,
  beginTwoFactorEnrollment,
  confirmTwoFactorEnrollment,
  disableTwoFactor,
  getTwoFactorStatus,
  regenerateTwoFactorRecoveryCodes,
} from "../../lib/api/client";

type Enrollment = Awaited<ReturnType<typeof beginTwoFactorEnrollment>>;

/** Manages authenticator enrollment, recovery codes, and secure 2FA removal. */
export function TwoFactorPanel() {
  const queryClient = useQueryClient();
  const status = useQuery({
    queryKey: ["auth", "two-factor"],
    queryFn: getTwoFactorStatus,
  });
  const [enrollment, setEnrollment] = useState<Enrollment>();
  const [confirmationCode, setConfirmationCode] = useState("");
  const [currentPassword, setCurrentPassword] = useState("");
  const [verificationCode, setVerificationCode] = useState("");
  const [recoveryCodes, setRecoveryCodes] = useState<string[]>();
  const [error, setError] = useState<string>();

  const begin = useMutation({
    mutationFn: beginTwoFactorEnrollment,
    onSuccess: (value) => {
      setEnrollment(value);
      setRecoveryCodes(undefined);
      setError(undefined);
    },
    onError: (reason) => setError(twoFactorError(reason)),
  });
  const confirm = useMutation({
    mutationFn: confirmTwoFactorEnrollment,
    onSuccess: async (value) => {
      setRecoveryCodes(value.recovery_codes);
      setEnrollment(undefined);
      setConfirmationCode("");
      setError(undefined);
      await queryClient.invalidateQueries({ queryKey: ["auth", "two-factor"] });
    },
    onError: (reason) => setError(twoFactorError(reason)),
  });
  const regenerate = useMutation({
    mutationFn: regenerateTwoFactorRecoveryCodes,
    onSuccess: async (value) => {
      setRecoveryCodes(value.recovery_codes);
      setCurrentPassword("");
      setVerificationCode("");
      setError(undefined);
      await queryClient.invalidateQueries({ queryKey: ["auth", "two-factor"] });
    },
    onError: (reason) => setError(twoFactorError(reason)),
  });
  const disable = useMutation({
    mutationFn: disableTwoFactor,
    onSuccess: async () => {
      await queryClient.invalidateQueries({
        queryKey: ["auth", "current-user"],
      });
    },
    onError: (reason) => setError(twoFactorError(reason)),
  });

  if (status.isPending) {
    return (
      <section className="two-factor-panel">
        Loading two-factor settings…
      </section>
    );
  }
  if (status.isError) {
    return (
      <section className="two-factor-panel notice danger">
        Two-factor settings are unavailable.
      </section>
    );
  }

  return (
    <section className="two-factor-panel" aria-labelledby="two-factor-heading">
      <div className="password-form-heading">
        <strong id="two-factor-heading">Two-factor authentication</strong>
        <span>
          Require an authenticator app or unused recovery code after your
          password.
        </span>
      </div>

      {recoveryCodes && (
        <RecoveryCodes
          codes={recoveryCodes}
          onDone={() => setRecoveryCodes(undefined)}
        />
      )}

      {!status.data.enabled && !enrollment && !recoveryCodes && (
        <div className="two-factor-callout">
          <span className="deployment-status">Not enabled</span>
          <p>Your account currently signs in with a password only.</p>
          <button
            className="primary-action"
            type="button"
            disabled={begin.isPending}
            onClick={() => begin.mutate()}
          >
            {begin.isPending ? "Preparing…" : "Set up authenticator"}
          </button>
        </div>
      )}

      {enrollment && !recoveryCodes && (
        <div className="two-factor-enrollment">
          <img
            src={qrSource(enrollment.qr_base64)}
            alt="Authenticator setup QR code"
          />
          <div>
            <strong>Scan this code</strong>
            <p>
              Scan with 1Password, Bitwarden, Google Authenticator, Authy, or
              another TOTP app. You can also enter this key manually:
            </p>
            <code>{enrollment.secret}</code>
            <label>
              Current six-digit code
              <input
                inputMode="numeric"
                autoComplete="one-time-code"
                value={confirmationCode}
                onChange={(event) => setConfirmationCode(event.target.value)}
              />
            </label>
            <div className="two-factor-actions">
              <button
                className="primary-action"
                type="button"
                disabled={
                  confirm.isPending || confirmationCode.trim().length !== 6
                }
                onClick={() => confirm.mutate(confirmationCode.trim())}
              >
                Confirm and enable
              </button>
              <button
                className="quiet-button"
                type="button"
                onClick={() => {
                  setEnrollment(undefined);
                  setConfirmationCode("");
                }}
              >
                Cancel
              </button>
            </div>
          </div>
        </div>
      )}

      {status.data.enabled && !recoveryCodes && (
        <div className="two-factor-enabled">
          <div className="two-factor-state">
            <span className="deployment-status active">Enabled</span>
            <span>
              {status.data.recovery_codes_remaining} recovery codes remain
            </span>
          </div>
          <p>
            Enter your password and a current authenticator or recovery code to
            regenerate recovery codes or disable 2FA.
          </p>
          <label>
            Current password
            <input
              type="password"
              autoComplete="current-password"
              value={currentPassword}
              onChange={(event) => setCurrentPassword(event.target.value)}
            />
          </label>
          <label>
            Authenticator or recovery code
            <input
              autoComplete="one-time-code"
              value={verificationCode}
              onChange={(event) => setVerificationCode(event.target.value)}
            />
          </label>
          <div className="two-factor-actions">
            <button
              className="table-action"
              type="button"
              disabled={
                regenerate.isPending ||
                !currentPassword ||
                !verificationCode.trim()
              }
              onClick={() =>
                regenerate.mutate({
                  current_password: currentPassword,
                  code: verificationCode.trim(),
                })
              }
            >
              Regenerate recovery codes
            </button>
            <button
              className="danger-action"
              type="button"
              disabled={
                disable.isPending ||
                !currentPassword ||
                !verificationCode.trim()
              }
              onClick={() =>
                disable.mutate({
                  current_password: currentPassword,
                  code: verificationCode.trim(),
                })
              }
            >
              Disable 2FA
            </button>
          </div>
        </div>
      )}

      {error && (
        <p className="notice danger" role="alert">
          {error}
        </p>
      )}
    </section>
  );
}

/** Displays one-time recovery codes with explicit save-before-dismiss guidance. */
function RecoveryCodes({
  codes,
  onDone,
}: {
  codes: string[];
  onDone: () => void;
}) {
  const [copied, setCopied] = useState(false);
  return (
    <div className="recovery-codes">
      <strong>Save these recovery codes now</strong>
      <p>
        Each code works once. Depot cannot show them again after you leave this
        screen.
      </p>
      <div>
        {codes.map((code) => (
          <code key={code}>{code}</code>
        ))}
      </div>
      <div className="two-factor-actions">
        <button
          className="table-action"
          type="button"
          onClick={async () => {
            await navigator.clipboard.writeText(codes.join("\n"));
            setCopied(true);
          }}
        >
          {copied ? "Copied" : "Copy codes"}
        </button>
        <button className="primary-action" type="button" onClick={onDone}>
          I saved them
        </button>
      </div>
    </div>
  );
}

/** Converts the backend QR payload into a browser-safe image source. */
function qrSource(value: string) {
  return value.startsWith("data:") ? value : `data:image/png;base64,${value}`;
}

/** Converts safe API failures into actionable account-setting feedback. */
function twoFactorError(reason: unknown) {
  return reason instanceof ApiError
    ? reason.message
    : "Depot could not update two-factor authentication.";
}
