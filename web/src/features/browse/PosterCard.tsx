import type { DiscoverTitle } from "../../api/types";
import type { LibraryState } from "./match";

/** One poster in a row. Artwork carries the meaning; text is supporting. */
export function PosterCard({
  title,
  library,
  onSelect,
}: {
  title: DiscoverTitle;
  library?: LibraryState;
  onSelect: (title: DiscoverTitle) => void;
}) {
  return (
    <button
      className="card"
      onClick={() => onSelect(title)}
      aria-label={`${title.title}${title.year ? `, ${title.year}` : ""}${
        library === "ready" ? ", in your library" : library === "downloading" ? ", downloading" : ""
      }`}
    >
      {/* Marked on the artwork rather than below it: the point is to be seen
          while scanning a row, before the title is even read. */}
      {library && (
        <span className={`owned owned-${library}`}>
          {library === "ready" ? "In library" : "Downloading"}
        </span>
      )}
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
          {title.media_kind === "series" && <span>Series</span>}
        </span>
      </span>
    </button>
  );
}
