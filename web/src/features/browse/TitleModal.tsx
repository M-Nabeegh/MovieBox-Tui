import { useEffect, useState } from "react";
import { api } from "../../api/client";
import type { DiscoverTitle, SourceOption } from "../../api/types";

type Match = { catalogId: string; sources: SourceOption[] };

/** Human-readable file size. */
function formatSize(bytes: number | null) {
  if (!bytes) return null;
  const gb = bytes / 1024 ** 3;
  return gb >= 1 ? `${gb.toFixed(1)} GB` : `${Math.round(bytes / 1024 ** 2)} MB`;
}

const QUALITY_COPY: Record<SourceOption["quality"], string> = {
  best: "Best",
  good: "Good",
  lower: "Low bitrate",
};

/**
 * Browsing runs on the metadata catalogue, but downloads come from the source
 * provider, so a chosen title has to be matched across before it can be queued.
 * The match is by title and year, which is what the provider indexes on.
 */
async function findSources(title: DiscoverTitle): Promise<Match> {
  const page = await api.catalog.search(title.title);
  const wanted = title.title.trim().toLowerCase();
  const exact = page.items.find(
    (item) =>
      item.title.trim().toLowerCase() === wanted &&
      (!title.year || !item.year || item.year === title.year),
  );
  const candidate = exact ?? page.items[0];
  if (!candidate) throw new Error("No downloadable source was found for this title.");
  const sources = await api.catalog.sources(candidate.id);
  return { catalogId: candidate.id, sources };
}

export function TitleModal({
  title,
  onClose,
  onQueued,
}: {
  title: DiscoverTitle;
  onClose: () => void;
  onQueued: () => void;
}) {
  const [match, setMatch] = useState<Match | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [queueing, setQueueing] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    setMatch(null);
    setError(null);
    findSources(title)
      .then((found) => active && setMatch(found))
      .catch((cause: Error) => active && setError(cause.message));
    return () => {
      active = false;
    };
  }, [title]);

  // Escape is the expected way out of a modal that covers the page.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => event.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  async function queue(source: SourceOption) {
    if (!match) return;
    setQueueing(source.id);
    setError(null);
    try {
      await api.createJob({
        catalog_id: match.catalogId,
        source_id: source.id,
        subtitle_id: null,
        requested_height: source.height,
      });
      onQueued();
      onClose();
    } catch (cause) {
      setError((cause as Error).message);
    } finally {
      setQueueing(null);
    }
  }

  return (
    <div
      className="modal-scrim"
      role="dialog"
      aria-modal="true"
      aria-label={title.title}
      onClick={(event) => event.target === event.currentTarget && onClose()}
    >
      <div className="modal">
        <div className="modal-art">
          {title.backdrop_url && <img src={title.backdrop_url} alt="" />}
          <button className="modal-close" onClick={onClose} aria-label="Close">
            ✕
          </button>
        </div>
        <div className="modal-body">
          <h2>{title.title}</h2>
          <div className="hero-meta">
            {title.year && <span>{title.year}</span>}
            {title.rating != null && (
              <span className="rating">{title.rating.toFixed(1)}</span>
            )}
            {title.language && <span>{title.language.toUpperCase()}</span>}
          </div>
          {title.overview && <p style={{ color: "var(--ink-soft)" }}>{title.overview}</p>}

          {error && <p className="error-note">{error}</p>}

          {!match && !error && <p className="empty">Finding sources…</p>}

          {match && (
            <div className="source-list">
              {match.sources.map((source) => (
                <button
                  key={source.id}
                  className="source"
                  onClick={() => queue(source)}
                  disabled={queueing !== null}
                >
                  <span className="source-label">{source.label}</span>
                  <span className={`badge badge-${source.quality}`}>
                    {QUALITY_COPY[source.quality]}
                  </span>
                  {source.recommended && <span className="badge badge-good">Recommended</span>}
                  <span className="source-size">
                    {queueing === source.id ? "Queueing…" : formatSize(source.size_bytes)}
                  </span>
                </button>
              ))}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
