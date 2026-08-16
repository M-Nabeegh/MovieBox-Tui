import type { DiscoverRow, DiscoverTitle } from "../../api/types";
import { PosterCard } from "./PosterCard";

export function Row({
  row,
  onSelect,
}: {
  row: DiscoverRow;
  onSelect: (title: DiscoverTitle) => void;
}) {
  if (row.items.length === 0) return null;
  return (
    <section aria-labelledby={`row-${row.id}`}>
      <div className="row-head">
        <h2 id={`row-${row.id}`}>{row.title}</h2>
        <span className="row-count">{row.items.length}</span>
      </div>
      <div className="row-scroller">
        {row.items.map((title) => (
          <PosterCard key={title.tmdb_id} title={title} onSelect={onSelect} />
        ))}
      </div>
    </section>
  );
}

/** Placeholder rows so the page has structure while artwork loads. */
export function RowSkeleton() {
  return (
    <section>
      <div className="row-head">
        <div className="skeleton" style={{ width: 180, height: 20 }} />
      </div>
      <div className="row-scroller">
        {Array.from({ length: 8 }, (_, index) => (
          <div key={index} className="skeleton skeleton-card" />
        ))}
      </div>
    </section>
  );
}
