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
      <div className="card-body">
        <p className="card-title">{title.title}</p>
        <div className="card-meta">
          {title.year && <span>{title.year}</span>}
          {title.rating != null && (
            <span className="rating">{title.rating.toFixed(1)}</span>
          )}
        </div>
      </div>
    </button>
  );
}
