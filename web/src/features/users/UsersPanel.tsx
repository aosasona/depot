import { useForm } from "@tanstack/react-form";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  type ColumnDef,
  type CoreFeatures,
  coreFeatures,
  flexRender,
  useTable,
} from "@tanstack/react-table";
import { useMemo, useState } from "react";
import { z } from "zod";
import {
  type AdminUser,
  ApiError,
  createUser,
  listUsers,
  updateUser,
} from "../../lib/api/client";

const createUserSchema = z.object({
  email: z.email("Enter a valid email address."),
  password: z.string().min(12, "Use at least 12 characters."),
  role: z.enum(["admin", "member"]),
});

/** Renders the real admin-only account list and account creation controls. */
export function UsersPanel() {
  const users = useQuery({ queryKey: ["users"], queryFn: listUsers });
  const queryClient = useQueryClient();
  const mutation = useMutation({
    mutationFn: updateUser,
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["users"] }),
  });
  const columns = useMemo<ColumnDef<CoreFeatures, AdminUser>[]>(
    () => [
      { accessorKey: "email", header: "Identity" },
      { accessorKey: "role", header: "Role" },
      {
        accessorKey: "active",
        header: "Status",
        cell: ({ row }) => (row.original.active ? "Active" : "Disabled"),
      },
      {
        id: "actions",
        header: "Action",
        cell: ({ row }) => (
          <button
            className="table-action"
            type="button"
            disabled={mutation.isPending}
            onClick={() =>
              mutation.mutate({
                id: row.original.id,
                role: row.original.role,
                active: !row.original.active,
              })
            }
          >
            {row.original.active ? "Disable" : "Enable"}
          </button>
        ),
      },
    ],
    [mutation],
  );
  const table = useTable({
    features: coreFeatures,
    data: users.data ?? [],
    columns,
  });

  return (
    <section className="users-panel" aria-labelledby="users-heading">
      <div className="section-heading">
        <div>
          <p className="eyebrow">ADMIN / ACCESS</p>
          <h2 id="users-heading">Users</h2>
        </div>
        <span className="count-block">{users.data?.length ?? 0} ACCOUNTS</span>
      </div>
      <CreateUserForm />
      {users.isError && (
        <p className="notice danger">User data is unavailable.</p>
      )}
      {mutation.error && (
        <p className="notice danger">{errorMessage(mutation.error)}</p>
      )}
      <div className="table-scroll">
        <table>
          <thead>
            {table.getHeaderGroups().map((group) => (
              <tr key={group.id}>
                {group.headers.map((header) => (
                  <th key={header.id}>
                    {flexRender(
                      header.column.columnDef.header,
                      header.getContext(),
                    )}
                  </th>
                ))}
              </tr>
            ))}
          </thead>
          <tbody>
            {table.getRowModel().rows.map((row) => (
              <tr key={row.id}>
                {row.getAllCells().map((cell) => (
                  <td key={cell.id}>
                    {flexRender(cell.column.columnDef.cell, cell.getContext())}
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </section>
  );
}

/** Creates a local account and refreshes the administration table. */
function CreateUserForm() {
  const queryClient = useQueryClient();
  const [error, setError] = useState<string>();
  const mutation = useMutation({ mutationFn: createUser });
  const form = useForm({
    defaultValues: {
      email: "",
      password: "",
      role: "member" as "admin" | "member",
    },
    onSubmit: async ({ value, formApi }) => {
      setError(undefined);
      const parsed = createUserSchema.safeParse(value);
      if (!parsed.success) return setError(parsed.error.issues[0]?.message);
      try {
        await mutation.mutateAsync(parsed.data);
        formApi.reset();
        await queryClient.invalidateQueries({ queryKey: ["users"] });
      } catch (reason) {
        setError(errorMessage(reason));
      }
    },
  });
  return (
    <form
      className="create-user-grid"
      onSubmit={(event) => {
        event.preventDefault();
        void form.handleSubmit();
      }}
    >
      <form.Field name="email">
        {(field) => (
          <input
            aria-label="New user email"
            placeholder="member@example.com"
            type="email"
            value={field.state.value}
            onChange={(event) => field.handleChange(event.target.value)}
          />
        )}
      </form.Field>
      <form.Field name="password">
        {(field) => (
          <input
            aria-label="Initial password"
            placeholder="Initial password"
            type="password"
            autoComplete="new-password"
            value={field.state.value}
            onChange={(event) => field.handleChange(event.target.value)}
          />
        )}
      </form.Field>
      <form.Field name="role">
        {(field) => (
          <select
            aria-label="New user role"
            value={field.state.value}
            onChange={(event) =>
              field.handleChange(event.target.value as "admin" | "member")
            }
          >
            <option value="member">Member</option>
            <option value="admin">Admin</option>
          </select>
        )}
      </form.Field>
      <button
        className="primary-action"
        type="submit"
        disabled={mutation.isPending}
      >
        Add user
      </button>
      {error && (
        <p className="notice danger" role="alert">
          {error}
        </p>
      )}
    </form>
  );
}

/** Converts unknown user-management failures into safe inline feedback. */
function errorMessage(reason: unknown) {
  return reason instanceof ApiError
    ? reason.message
    : "Depot could not update this account.";
}
