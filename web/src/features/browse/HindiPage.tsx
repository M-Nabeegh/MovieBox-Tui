import { useCallback, useEffect, useState } from "react";
import { api } from "../../api/client";
import type { DiscoverTitle, Job } from "../../api/types";
import { libraryStateFor } from "./match";
import { PosterCard } from "./PosterCard";

/** Years offered as filters, newest first, plus an unfiltered option. */
function yearChoices(current: number): (number | null)[] {
  const years: (number | null)[] = [null];
  for (let year = current; year >= current - 9; year -= 1) years.push(year);
  return years;
}

/**
 * A section devoted to one language.
 *
 * Rows are good for grazing but bad for looking something up: they show twenty
 * titles and hide the rest. This is a grid instead, narrowed by year, for when
 * you know the kind of thing you want.
 */
export function HindiPage({
  onSelect,
}: {
  onSelect: (title: DiscoverTitle) => void;
}) {
  const currentYear = new Date().getFullYear();
  const [year, setYear] = useState<number | null>(null);
  const [kind, setKind] = useState<"movie" | "series">("movie");
  const [titles, setTitles] = useState<DiscoverTitle[] | null>(null);
  const [jobs, setJobs] = useState<Job[]>([]);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api.jobs.list().then(setJobs).catch(() => setJobs([]));
  }, []);

  useEffect(() => {
    let active = true;
    setTitles(null);
    setError(null);
    api.discover
      .filter({ language: "hi", year, kind })
      .then((found) => active && setTitles(found))
      .catch((cause: Error) => active && setError(cause.message));
    return () => {
      active = false;
    };
  }, [year, kind]);

  const libraryFor = useCallback(
    (title: DiscoverTitle) => libraryStateFor(title, jobs),
    [jobs],
  );

  return (
    <section className="section">
      <div className="section-head">
        <h1>Hindi</h1>
        <p className="card-meta">Films and shows in Hindi, newest first.</p>
      </div>

      <div className="filters" role="group" aria-label="Filter Hindi titles">
        <div className="filter-group">
          {(["movie", "series"] as const).map((option) => (
            <button
              key={option}
              className="chip"
              aria-pressed={kind === option}
              onClick={() => setKind(option)}
            >
              {option === "movie" ? "Films" : "Shows"}
            </button>
          ))}
        </div>

        <div className="filter-group">
          {yearChoices(currentYear).map((option) => (
            <button
              key={option ?? "all"}
              className="chip"
              aria-pressed={year === option}
              onClick={() => setYear(option)}
            >
              {option ?? "All years"}
            </button>
          ))}
        </div>
      </div>

      {error && <p className="empty">Browsing is unavailable right now. {error}</p>}
      {!titles && !error && <p className="empty">Loading…</p>}
      {titles && titles.length === 0 && (
        <p className="empty">
          Nothing found for {kind === "movie" ? "films" : "shows"}
          {year ? ` from ${year}` : ""}.
        </p>
      )}

      {titles && titles.length > 0 && (
        <div className="poster-grid">
          {titles.map((title) => (
            <PosterCard
              key={title.tmdb_id}
              title={title}
              library={libraryFor(title)}
              onSelect={onSelect}
            />
          ))}
        </div>
      )}
    </section>
  );
}
