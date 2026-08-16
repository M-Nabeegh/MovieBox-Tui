import { useEffect, useState } from "react";
import { api, setUnauthorizedHandler } from "./api/client";
import type { DiscoverTitle, Session } from "./api/types";
import { LoginPage } from "./features/auth/LoginPage";
import { BrowsePage } from "./features/browse/BrowsePage";
import { TitleModal } from "./features/browse/TitleModal";
import { QueuePage } from "./features/queue/QueuePage";

type View = "browse" | "queue";

function TopNav({
  view,
  setView,
  query,
  setQuery,
  signOut,
}: {
  view: View;
  setView: (view: View) => void;
  query: string;
  setQuery: (query: string) => void;
  signOut: () => Promise<void>;
}) {
  const [scrolled, setScrolled] = useState(false);

  // The bar is transparent over the hero and solid once the artwork scrolls
  // away, so nothing covers the banner on arrival.
  useEffect(() => {
    const onScroll = () => setScrolled(window.scrollY > 24);
    onScroll();
    window.addEventListener("scroll", onScroll, { passive: true });
    return () => window.removeEventListener("scroll", onScroll);
  }, []);

  return (
    <header className="topnav" data-scrolled={scrolled}>
      <span className="wordmark">
        NABEEGH<em>BOX</em>
      </span>
      <nav>
        <button aria-current={view === "browse"} onClick={() => setView("browse")}>
          Browse
        </button>
        <button aria-current={view === "queue"} onClick={() => setView("queue")}>
          Downloads
        </button>
      </nav>
      <div className="nav-actions">
        <label className="search-field">
          <span aria-hidden="true">⌕</span>
          <input
            value={query}
            onChange={(event) => {
              setQuery(event.target.value);
              setView("browse");
            }}
            placeholder="Titles, people, genres"
            aria-label="Search"
          />
        </label>
        <button className="btn btn-quiet" onClick={signOut}>
          Sign out
        </button>
      </div>
    </header>
  );
}

function Shell({ session, signOut }: { session: Session; signOut: () => Promise<void> }) {
  const [view, setView] = useState<View>("browse");
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState<DiscoverTitle | null>(null);
  const [queueVersion, setQueueVersion] = useState(0);

  return (
    <main>
      <TopNav
        view={view}
        setView={setView}
        query={query}
        setQuery={setQuery}
        signOut={signOut}
      />
      {view === "browse" ? (
        <BrowsePage name={session.display_name} query={query} onSelect={setSelected} />
      ) : (
        <div style={{ paddingTop: 96 }}>
          <QueuePage key={queueVersion} />
        </div>
      )}
      {selected && (
        <TitleModal
          title={selected}
          onClose={() => setSelected(null)}
          onQueued={() => {
            setQueueVersion((version) => version + 1);
            setView("queue");
          }}
        />
      )}
    </main>
  );
}

export default function App() {
  const [session, setSession] = useState<Session | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    setUnauthorizedHandler(() => setSession(null));
    api
      .session()
      .then(setSession)
      .catch(() => setSession(null))
      .finally(() => setLoading(false));
  }, []);

  async function signOut() {
    await api.request<void>("/auth/logout", { method: "POST" });
    setSession(null);
  }

  if (loading) return <div className="loading">Warming the projector…</div>;
  return session ? (
    <Shell session={session} signOut={signOut} />
  ) : (
    <LoginPage onAuthenticated={async () => setSession(await api.session())} />
  );
}
