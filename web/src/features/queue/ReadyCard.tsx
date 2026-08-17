import { useEffect, useState } from "react";
import { api } from "../../api/client";
import type { Job, LibraryStatus } from "../../api/types";

/**
 * A finished download, with the link that actually plays it.
 *
 * The file being on disk is not the end of the job — the point of downloading
 * was to watch it, so the card carries the media server's own link rather than
 * leaving the user to go and find it.
 */
export function ReadyCard({ job }: { job: Job }) {
  const [library, setLibrary] = useState<LibraryStatus | null>(null);

  useEffect(() => {
    let active = true;
    api.library
      .forJob(job.id)
      .then((status) => active && setLibrary(status))
      .catch(() => active && setLibrary({ status: "unavailable", url: null }));
    return () => {
      active = false;
    };
  }, [job.id]);

  return (
    <article className="job" aria-label={`${job.title}: ready to watch`}>
      <div className="job-head">
        <span className="job-title">{job.title}</span>
        <span className="card-meta">
          {job.year ?? "—"} · {job.requested_height}p
        </span>
        <span className="job-state" style={{ color: "var(--positive)" }}>
          Ready to watch
        </span>
      </div>

      {job.warning && (
        <p className="card-meta" role="status">
          {job.warning}
        </p>
      )}

      <div className="job-actions">
        {library?.url ? (
          // Opened in a new tab: the queue is worth keeping in place while a
          // separate player takes over.
          <a
            className="btn btn-primary"
            href={library.url}
            target="_blank"
            rel="noreferrer noopener"
          >
            ▶ Play in nabeeghfin
          </a>
        ) : (
          <span className="card-meta">
            {library === null
              ? "Checking your library…"
              : library.status === "scan_pending"
                ? "Waiting for nabeeghfin to index it."
                : "Not indexed by nabeeghfin yet."}
          </span>
        )}
      </div>
    </article>
  );
}
