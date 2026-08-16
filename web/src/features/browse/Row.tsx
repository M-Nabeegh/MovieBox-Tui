import { useCallback, useEffect, useRef, useState } from "react";
import type { DiscoverRow, DiscoverTitle } from "../../api/types";
import { PosterCard } from "./PosterCard";

/** How much of the visible width one arrow press moves, as a fraction. */
const PAGE_FRACTION = 0.9;

export function Row({
  row,
  onSelect,
}: {
  row: DiscoverRow;
  onSelect: (title: DiscoverTitle) => void;
}) {
  const scroller = useRef<HTMLDivElement>(null);
  const [atStart, setAtStart] = useState(true);
  const [atEnd, setAtEnd] = useState(false);

  // Arrows are only shown where there is somewhere to go, so the row does not
  // advertise a direction that does nothing.
  const measure = useCallback(() => {
    const element = scroller.current;
    if (!element) return;
    const max = element.scrollWidth - element.clientWidth;
    setAtStart(element.scrollLeft <= 1);
    setAtEnd(element.scrollLeft >= max - 1);
  }, []);

  useEffect(() => {
    measure();
    const element = scroller.current;
    if (!element) return;
    // Re-measure when the viewport changes: a wider window can make a
    // previously scrollable row fit entirely.
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
  }, [measure, row.items.length]);

  function page(direction: -1 | 1) {
    const element = scroller.current;
    if (!element) return;
    element.scrollBy({
      left: direction * element.clientWidth * PAGE_FRACTION,
      behavior: "smooth",
    });
  }

  if (row.items.length === 0) return null;

  return (
    <section className="row" aria-labelledby={`row-${row.id}`}>
      <div className="row-head">
        <h2 id={`row-${row.id}`}>{row.title}</h2>
        <span className="row-count">{row.items.length}</span>
      </div>

      <div className="row-viewport">
        {!atStart && (
          <button
            className="row-arrow row-arrow-prev"
            onClick={() => page(-1)}
            aria-label={`Scroll ${row.title} left`}
          >
            <span aria-hidden="true">‹</span>
          </button>
        )}

        <div className="row-scroller" ref={scroller} onScroll={measure}>
          {row.items.map((title) => (
            <PosterCard key={title.tmdb_id} title={title} onSelect={onSelect} />
          ))}
        </div>

        {!atEnd && (
          <button
            className="row-arrow row-arrow-next"
            onClick={() => page(1)}
            aria-label={`Scroll ${row.title} right`}
          >
            <span aria-hidden="true">›</span>
          </button>
        )}
      </div>
    </section>
  );
}

/** Placeholder rows so the page has structure while artwork loads. */
export function RowSkeleton() {
  return (
    <section className="row">
      <div className="row-head">
        <div className="skeleton" style={{ width: 180, height: 20 }} />
      </div>
      <div className="row-viewport">
        <div className="row-scroller">
          {Array.from({ length: 8 }, (_, index) => (
            <div key={index} className="skeleton skeleton-card" />
          ))}
        </div>
      </div>
    </section>
  );
}
