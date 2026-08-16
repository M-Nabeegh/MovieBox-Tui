import { FormEvent, useState } from "react";
import { api } from "../../api/client";

type Props = { onAuthenticated: () => Promise<void> | void };

export function LoginPage({ onAuthenticated }: Props) {
  const [password, setPassword] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);

  async function submit(event: FormEvent) {
    event.preventDefault();
    setError("");
    setBusy(true);
    try {
      await api.request<void>("/auth/login", {
        method: "POST",
        body: JSON.stringify({ password }),
      });
      await api.session();
      await onAuthenticated();
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : "Sign in failed.");
    } finally {
      setBusy(false);
    }
  }

  return (
    <main className="login">
      <form onSubmit={submit}>
        <span className="wordmark">
          NABEEGH<em>BOX</em>
        </span>
        <h1>Sign in</h1>
        <label htmlFor="password" style={{ color: "var(--ink-muted)", fontSize: "var(--step-small)" }}>
          Server password
        </label>
        <input
          id="password"
          name="password"
          type="password"
          autoComplete="current-password"
          value={password}
          onChange={(event) => setPassword(event.target.value)}
          required
        />
        {error && (
          <p className="error-note" role="alert">
            {error}
          </p>
        )}
        <button className="btn btn-primary" type="submit" disabled={busy}>
          {busy ? "Checking…" : "Sign in"}
        </button>
        <p style={{ color: "var(--ink-faint)", fontSize: "var(--step-tiny)", margin: 0 }}>
          Your session stays in a secure cookie on this device.
        </p>
      </form>
    </main>
  );
}
