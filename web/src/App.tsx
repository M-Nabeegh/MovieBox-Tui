import { useEffect, useState } from "react";
import { api, setUnauthorizedHandler } from "./api/client";
import type { Session } from "./api/types";
import { LoginPage } from "./features/auth/LoginPage";

function Shell({ session, signOut }: { session: Session; signOut: () => Promise<void> }) {
  return <main className="app-shell"><header><div className="brand"><span className="brand-mark">MB</span><span>MovieBox <small>SERVER</small></span></div><button className="quiet-button" onClick={signOut}>Sign out</button></header>
    <section className="hero"><p className="eyebrow">GOOD EVENING, {session.username.toUpperCase()}</p><h1>What are we<br /><em>watching next?</em></h1><p>Search the catalog, choose a source, and keep your library moving.</p></section>
    <section className="search-panel" role="search" aria-label="Search"><label htmlFor="search">Find a film or series</label><div className="search-row"><input id="search" placeholder="Try “The Bear” or “Dune”" /><button type="button">Search <span aria-hidden="true">↗</span></button></div></section>
    <div className="shell-grid"><article><p className="eyebrow">CATALOG</p><h2>Ready when you are.</h2><p className="muted">Search results will appear here. The server resolves sources privately and caps downloads at 1080p.</p></article><aside><p className="eyebrow">QUEUE</p><strong>0</strong><p className="muted">active downloads</p></aside></div>
  </main>;
}

export default function App() {
  const [session, setSession] = useState<Session | null>(null); const [loading, setLoading] = useState(true);
  useEffect(() => { setUnauthorizedHandler(() => setSession(null)); api.session().then(setSession).catch(() => setSession(null)).finally(() => setLoading(false)); }, []);
  async function signOut() { await api.request<void>("/auth/logout", { method: "POST" }); setSession(null); }
  if (loading) return <div className="loading">Warming the projector…</div>;
  return session ? <Shell session={session} signOut={signOut} /> : <LoginPage onAuthenticated={async () => setSession(await api.session())} />;
}
