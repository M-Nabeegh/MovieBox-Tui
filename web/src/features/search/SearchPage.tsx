import { useEffect, useRef, useState } from "react";
import { api } from "../../api/client";
import type { CatalogItem, SearchPage as SearchResponse } from "../../api/types";
import { ResultCard } from "./ResultCard";

export function SearchPage({ onSelect }: { onSelect: (item: CatalogItem) => void }) {
  const [query, setQuery] = useState("");
  const [results, setResults] = useState<CatalogItem[]>([]);
  const [status, setStatus] = useState<"idle" | "loading" | "empty" | "error">("idle");
  const request = useRef<AbortController | null>(null);

  useEffect(() => {
    const normalized = query.trim().replace(/\s+/g, " ");
    request.current?.abort();
    if (normalized.length < 2) { setResults([]); setStatus("idle"); return; }
    const controller = new AbortController();
    request.current = controller;
    const timer = window.setTimeout(() => {
      setStatus("loading");
      api.request<SearchResponse>(`/catalog/search?q=${encodeURIComponent(normalized)}&page=1`, { signal: controller.signal })
        .then((page) => { if (!controller.signal.aborted) { setResults(page.items); setStatus(page.items.length ? "idle" : "empty"); } })
        .catch((error: unknown) => { if (!controller.signal.aborted) setStatus("error"); });
    }, 300);
    return () => { window.clearTimeout(timer); controller.abort(); };
  }, [query]);

  return <section className="catalog-panel" role="search" aria-label="Catalog results"><div className="search-row"><input aria-label="Find a film or series" placeholder="Try “The Bear” or “Dune”" value={query} onChange={(event) => setQuery(event.target.value)} /><button type="button" onClick={() => setQuery(query.trim())}>Search <span aria-hidden="true">↗</span></button></div>
    <div className="results-heading"><div><p className="eyebrow">CATALOG</p><h2>{query.trim() ? "Search results" : "Ready when you are."}</h2></div>{status === "loading" && <span className="muted">Searching…</span>}</div>
    {status === "error" && <p className="form-error" role="alert">Search is unavailable right now. Try again.</p>}
    {status === "empty" && <p className="muted">No titles matched “{query.trim()}”.</p>}
    {status === "idle" && results.length === 0 && <p className="muted">Search the catalog to choose a movie or series. The server keeps provider URLs private.</p>}
    <div className="result-list">{results.map((item) => <ResultCard key={item.id} item={item} onSelect={onSelect} />)}</div>
  </section>;
}
