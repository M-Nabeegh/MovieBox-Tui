import { useEffect, useState } from "react";
import { api } from "../../api/client";
import type { DiscoverRow, DiscoverTitle } from "../../api/types";
import { Hero } from "./Hero";
import { Row, RowSkeleton } from "./Row";

export function BrowsePage({
  name,
  query,
  onSelect,
}: {
  name: string;
  query: string;
  onSelect: (title: DiscoverTitle) => void;
}) {
  const [rows, setRows] = useState<DiscoverRow[] | null>(null);
  const [results, setResults] = useState<DiscoverTitle[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api.discover
      .rows()
      .then(setRows)
      .catch((cause: Error) => setError(cause.message));
  }, []);

  // Searching replaces the rows; clearing the box restores them.
  useEffect(() => {
    const term = query.trim();
    if (term.length < 2) {
      setResults(null);
      return;
    }
    let active = true;
    const timer = window.setTimeout(() => {
      api.discover
        .search(term)
        .then((found) => active && setResults(found))
        .catch(() => active && setResults([]));
    }, 250);
    return () => {
      active = false;
      window.clearTimeout(timer);
    };
  }, [query]);

  if (error) {
    return (
      <p className="empty">
        Browsing is unavailable right now. {error}
      </p>
    );
  }

  if (results) {
    return (
      <div className="rows" style={{ marginTop: 96 }}>
        <Row
          row={{ id: "results", title: `Results for “${query.trim()}”`, items: results }}
          onSelect={onSelect}
        />
        {results.length === 0 && <p className="empty">Nothing matched that search.</p>}
      </div>
    );
  }

  if (!rows) {
    return (
      <div className="rows" style={{ marginTop: 96 }}>
        <RowSkeleton />
        <RowSkeleton />
        <RowSkeleton />
      </div>
    );
  }

  // The hero borrows the first trending title, which is the one with artwork
  // most likely to be wide enough to fill the banner.
  const featured = rows[0]?.items.find((item) => item.backdrop_url) ?? null;

  return (
    <>
      <Hero title={featured} name={name} onSelect={onSelect} />
      <div className="rows">
        {rows.map((row) => (
          <Row key={row.id} row={row} onSelect={onSelect} />
        ))}
      </div>
    </>
  );
}
