import { useEffect, useState } from "react";
import { api, setUnauthorizedHandler } from "./api/client";
import type { Session } from "./api/types";
import { LoginPage } from "./features/auth/LoginPage";
import { SearchPage } from "./features/search/SearchPage";
import { DetailsDrawer } from "./features/details/DetailsDrawer";
import type { CatalogItem } from "./api/types";

function Shell({ session, signOut }: { session: Session; signOut: () => Promise<void> }) {
  const [selected, setSelected] = useState<CatalogItem | null>(null);
  const [queued, setQueued] = useState(0);
  return <main className="app-shell"><header><div className="brand"><span className="brand-mark">MB</span><span>MovieBox <small>SERVER</small></span></div><button className="quiet-button" onClick={signOut}>Sign out</button></header>
    <section className="hero"><p className="eyebrow">GOOD EVENING, {session.username.toUpperCase()}</p><h1>What are we<br /><em>watching next?</em></h1><p>Search the catalog, choose a source, and keep your library moving.</p></section>
    <div className="shell-grid"><SearchPage onSelect={setSelected} /><aside><p className="eyebrow">QUEUE</p><strong>{queued}</strong><p className="muted">active downloads</p></aside></div>
    {selected && <DetailsDrawer item={selected} onClose={() => setSelected(null)} onQueued={() => setQueued((count) => count + 1)} />}
  </main>;
}

export default function App() {
  const [session, setSession] = useState<Session | null>(null); const [loading, setLoading] = useState(true);
  useEffect(() => { setUnauthorizedHandler(() => setSession(null)); api.session().then(setSession).catch(() => setSession(null)).finally(() => setLoading(false)); }, []);
  async function signOut() { await api.request<void>("/auth/logout", { method: "POST" }); setSession(null); }
  if (loading) return <div className="loading">Warming the projector…</div>;
  return session ? <Shell session={session} signOut={signOut} /> : <LoginPage onAuthenticated={async () => setSession(await api.session())} />;
}
