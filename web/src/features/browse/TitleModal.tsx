import { useCallback, useEffect, useState } from "react";
import { api } from "../../api/client";
import type { CatalogDetails, DiscoverTitle, SourceOption } from "../../api/types";
import { findMatch } from "./match";

type Match = {
  catalogId: string;
  matchedTitle: string;
  matchedYear: string | null;
  /** What the download catalogue itself says about the matched entry. */
  details: CatalogDetails | null;
};

type Episode = { season: number; episode: number };

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
 *
 * If no confident match exists the request fails. Falling back to the closest
 * search result would silently download a different film — a franchise sibling
 * is not a near-miss, it is the wrong movie.
 */
async function findEntry(title: DiscoverTitle): Promise<Match> {
  const page = await api.catalog.search(title.title);
  const matched = findMatch(title, page.items);
  if (!matched) {
    throw new Error(
      `“${title.title}”${title.year ? ` (${title.year})` : ""} is not in the download catalogue yet.`,
    );
  }
  const details = await api.catalog.details(matched.id).catch(() => null);
  return {
    catalogId: matched.id,
    matchedTitle: matched.title,
    matchedYear: matched.year,
    details,
  };
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
  const isSeries = title.media_kind === "series";
  const [match, setMatch] = useState<Match | null>(null);
  const [sources, setSources] = useState<SourceOption[] | null>(null);
  const [episode, setEpisode] = useState<Episode | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [queueing, setQueueing] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    setMatch(null);
    setSources(null);
    setEpisode(null);
    setError(null);
    findEntry(title)
      .then((found) => active && setMatch(found))
      .catch((cause: Error) => active && setError(cause.message));
    return () => {
      active = false;
    };
  }, [title]);

  const loadSources = useCallback(
    (chosen: Episode | null) => {
      if (!match) return;
      setSources(null);
      setError(null);
      api.catalog
        .sources(match.catalogId, chosen ?? undefined)
        .then(setSources)
        .catch(() => setError("No sources are available for this selection."));
    },
    [match],
  );

  // A film has one set of sources; a show has one per episode, so nothing is
  // fetched until an episode is chosen.
  useEffect(() => {
    if (!match) return;
    if (!isSeries) loadSources(null);
  }, [match, isSeries, loadSources]);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => event.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  function chooseEpisode(chosen: Episode) {
    setEpisode(chosen);
    loadSources(chosen);
  }

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
        // A show downloads one episode at a time, so the queued job has to say
        // which one; a film carries neither field.
        ...(episode ? { season: episode.season, episode: episode.episode } : {}),
      });
      onQueued();
      onClose();
    } catch (cause) {
      setError((cause as Error).message);
    } finally {
      setQueueing(null);
    }
  }

  const seasons = match?.details?.seasons ?? [];

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
          {/* Not every title has a landscape still; the poster is better than a
              black rectangle. */}
          {(title.backdrop_url || title.poster_url) && (
            <img src={title.backdrop_url ?? title.poster_url ?? ""} alt="" />
          )}
          <button className="modal-close" onClick={onClose} aria-label="Close">
            ✕
          </button>
        </div>
        <div className="modal-body">
          <h2>{title.title}</h2>
          <div className="hero-meta">
            {title.year && <span>{title.year}</span>}
            {title.rating != null && <span className="rating">{title.rating.toFixed(1)}</span>}
            {title.language && <span>{title.language.toUpperCase()}</span>}
            <span className="badge badge-good">{isSeries ? "Series" : "Film"}</span>
          </div>
          {title.overview && <p style={{ color: "var(--ink-soft)" }}>{title.overview}</p>}

          {error && <p className="error-note">{error}</p>}
          {!match && !error && <p className="empty">Finding this title…</p>}

          {match && (
            <>
              {/* Show the matched entry as the download catalogue describes it.
                  A wrong match is only dangerous while it stays invisible. */}
              <div className="verify">
                {title.poster_url && <img className="verify-art" src={title.poster_url} alt="" />}
                <div>
                  <p className="verify-label">About to download</p>
                  <p className="verify-title">
                    {match.matchedTitle}
                    {match.matchedYear ? ` (${match.matchedYear})` : ""}
                  </p>
                  {match.details && (
                    <p className="card-meta">
                      {[
                        match.details.duration,
                        match.details.genres.slice(0, 3).join(", ") || null,
                        match.details.country,
                        match.details.imdb_rating ? `IMDb ${match.details.imdb_rating}` : null,
                      ]
                        .filter(Boolean)
                        .join(" · ")}
                    </p>
                  )}
                  {match.details?.description && (
                    <p className="verify-synopsis">{match.details.description}</p>
                  )}
                </div>
              </div>

              {isSeries && seasons.length > 0 && (
                <div className="seasons">
                  {seasons.map((season) => (
                    <div key={season.number}>
                      <p className="verify-label">Season {season.number}</p>
                      <div className="episode-grid">
                        {season.episodes.map((item) => {
                          const chosen =
                            episode?.season === season.number && episode?.episode === item.number;
                          return (
                            <button
                              key={`${season.number}-${item.number}`}
                              className="episode"
                              aria-pressed={chosen}
                              onClick={() =>
                                chooseEpisode({ season: season.number, episode: item.number })
                              }
                            >
                              <span className="episode-number">E{item.number}</span>
                              {item.title && <span className="episode-title">{item.title}</span>}
                            </button>
                          );
                        })}
                      </div>
                    </div>
                  ))}
                </div>
              )}

              {isSeries && seasons.length === 0 && (
                <p className="empty">This show has no episodes listed yet.</p>
              )}

              {isSeries && !episode && seasons.length > 0 && (
                <p className="empty">Choose an episode to see its download options.</p>
              )}

              {(!isSeries || episode) && !sources && !error && (
                <p className="empty">Finding sources…</p>
              )}

              {sources && (
                <div className="source-list">
                  {sources.map((source) => (
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
            </>
          )}
        </div>
      </div>
    </div>
  );
}
