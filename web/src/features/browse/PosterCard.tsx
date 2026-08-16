import type { DiscoverTitle } from "../../api/types";

/** One poster in a row. Artwork carries the meaning; text is supporting. */
export function PosterCard({
  title,
  onSelect,
}: {
  title: DiscoverTitle;
  onSelect: (title: DiscoverTitle) => void;
}) {
  return (
    <button
      className="card"
      onClick={() => onSelect(title)}
      aria-label={`${title.title}${title.year ? `, ${title.year}` : ""}`}
    >
      {title.poster_url && (
        <img
          className="card-art"
          src={title.poster_url}
          alt=""
          loading="lazy"
          decoding="async"
        />
      )}
      {/* Spans, not paragraphs: a button may only contain phrasing content. */}
      <span className="card-body">
        <span className="card-title">{title.title}</span>
        <span className="card-meta">
          {title.year && <span>{title.year}</span>}
          {title.rating != null && (
            <span className="rating">{title.rating.toFixed(1)}</span>
          )}
        </span>
      </span>
    </button>
  );
}
