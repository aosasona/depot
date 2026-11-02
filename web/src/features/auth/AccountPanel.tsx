import { useForm } from "@tanstack/react-form";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { z } from "zod";
import { ApiError, changePassword, type User } from "../../lib/api/client";
import { TwoFactorPanel } from "./TwoFactorPanel";

const passwordSchema = z
  .object({
    currentPassword: z.string().min(1, "Enter your current password."),
    newPassword: z.string().min(12, "Use at least 12 characters."),
    confirmPassword: z.string(),
  })
  .refine((value) => value.newPassword === value.confirmPassword, {
    path: ["confirmPassword"],
    message: "The new passwords do not match.",
  });

/** Lets an authenticated account rotate its local credential safely. */
export function AccountPanel({ user }: { user: User }) {
  const queryClient = useQueryClient();
  const [error, setError] = useState<string>();
  const mutation = useMutation({ mutationFn: changePassword });
  const form = useForm({
    defaultValues: {
      currentPassword: "",
      newPassword: "",
      confirmPassword: "",
    },
    onSubmit: async ({ value }) => {
      setError(undefined);
      const parsed = passwordSchema.safeParse(value);
      if (!parsed.success) return setError(parsed.error.issues[0]?.message);
      try {
        await mutation.mutateAsync({
          current_password: parsed.data.currentPassword,
          new_password: parsed.data.newPassword,
        });
        await queryClient.invalidateQueries({
          queryKey: ["auth", "current-user"],
        });
      } catch (reason) {
        setError(
          reason instanceof ApiError
            ? reason.message
            : "Depot could not change your password.",
        );
      }
    },
  });
  return (
    <section className="account-panel" aria-labelledby="account-heading">
      <div className="account-summary">
        <span className="account-avatar">{user.email[0]?.toUpperCase()}</span>
        <div>
          <h2 id="account-heading">Local account</h2>
          <p>{user.email}</p>
        </div>
        <span className="deployment-status active">{user.role}</span>
      </div>
      <div className="account-security-stack">
        <form
          className="password-form"
          onSubmit={(event) => {
            event.preventDefault();
            void form.handleSubmit();
          }}
        >
          <div className="password-form-heading">
            <strong>Change password</strong>
            <span>
              Changing it signs this account out everywhere, including this
              browser.
            </span>
          </div>
          <form.Field name="currentPassword">
            {(field) => (
              <label>
                Current password
                <input
                  type="password"
                  autoComplete="current-password"
                  value={field.state.value}
                  onChange={(event) => field.handleChange(event.target.value)}
                />
              </label>
            )}
          </form.Field>
          <form.Field name="newPassword">
            {(field) => (
              <label>
                New password
                <input
                  type="password"
                  autoComplete="new-password"
                  value={field.state.value}
                  onChange={(event) => field.handleChange(event.target.value)}
                />
              </label>
            )}
          </form.Field>
          <form.Field name="confirmPassword">
            {(field) => (
              <label>
                Confirm new password
                <input
                  type="password"
                  autoComplete="new-password"
                  value={field.state.value}
                  onChange={(event) => field.handleChange(event.target.value)}
                />
              </label>
            )}
          </form.Field>
          {error && (
            <p className="notice danger" role="alert">
              {error}
            </p>
          )}
          <button
            className="primary-action"
            type="submit"
            disabled={mutation.isPending}
          >
            {mutation.isPending ? "Changing password…" : "Change password"}
          </button>
        </form>
        <TwoFactorPanel />
      </div>
    </section>
  );
}
