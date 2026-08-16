import type { DiscoverTitle } from "../../api/types";

/** The greeting the user sees first. Time-aware so it reads as written for them. */
function greetingFor(date: Date) {
  const hour = date.getHours();
  if (hour < 5) return "Still up";
  if (hour < 12) return "Good morning";
  if (hour < 17) return "Good afternoon";
  return "Good evening";
}

export function Hero({
  title,
  name,
  onSelect,
}: {
  title: DiscoverTitle | null;
  name: string;
  onSelect: (title: DiscoverTitle) => void;
}) {
  if (!title) return null;
  return (
    <header className="hero">
      {title.backdrop_url && (
        <img className="hero-art" src={title.backdrop_url} alt="" fetchPriority="high" />
      )}
      <div className="hero-body">
        <p className="greeting">
          {greetingFor(new Date())}, <strong>{name}</strong> — what would you like to
          watch today?
        </p>
        <h1>{title.title}</h1>
        <div className="hero-meta">
          {title.year && <span>{title.year}</span>}
          {title.rating != null && (
            <span className="rating">{title.rating.toFixed(1)}</span>
          )}
          {title.language && <span>{title.language.toUpperCase()}</span>}
        </div>
        {title.overview && <p className="hero-synopsis">{title.overview}</p>}
        <div className="hero-actions">
          <button className="btn btn-primary" onClick={() => onSelect(title)}>
            Download
          </button>
          <button className="btn btn-ghost" onClick={() => onSelect(title)}>
            More info
          </button>
        </div>
      </div>
    </header>
  );
}
