import { FormEvent, useState } from "react";
import { api } from "../../api/client";

type Props = { onAuthenticated: () => Promise<void> | void };

export function LoginPage({ onAuthenticated }: Props) {
  const [password, setPassword] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  async function submit(event: FormEvent) {
    event.preventDefault(); setError(""); setBusy(true);
    try { await api.request<void>("/auth/login", { method: "POST", body: JSON.stringify({ password }) }); await api.session(); await onAuthenticated(); }
    catch (reason) { setError(reason instanceof Error ? reason.message : "Sign in failed."); }
    finally { setBusy(false); }
  }
  return <main className="login-page"><div className="login-card">
    <p className="eyebrow">MOVIEBOX / PRIVATE SERVER</p>
    <h1>Your cinema,<br /><em>on your terms.</em></h1>
    <p className="lede">Sign in to search the catalog and send titles to your home library.</p>
    <form onSubmit={submit}>
      <label htmlFor="password">Server password</label>
      <input id="password" name="password" type="password" autoComplete="current-password" value={password} onChange={(e) => setPassword(e.target.value)} required />
      {error && <p className="form-error" role="alert">{error}</p>}
      <button type="submit" disabled={busy}>{busy ? "Checking…" : "Sign in"}<span aria-hidden="true">↗</span></button>
    </form>
    <p className="privacy-note">Session stays in a secure browser cookie.<br />Your password never leaves this sign-in request.</p>
  </div><div className="login-mark" aria-hidden="true"><span>MB</span></div></main>;
}
