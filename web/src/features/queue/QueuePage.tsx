import { useCallback, useEffect, useState } from "react";
import { connectToJobEvents } from "../../api/events";
import { api } from "../../api/client";
import type { Job } from "../../api/types";
import { JobCard } from "./JobCard";

export function QueuePage({
  pollIntervalMs = 30_000,
  reconnectDelaysMs,
}: { pollIntervalMs?: number; reconnectDelaysMs?: number[] } = {}) {
  const [jobs, setJobs] = useState<Job[]>([]);
  const [error, setError] = useState("");

  const refresh = useCallback(
    () =>
      api.jobs
        .list()
        .then((next) => {
          setJobs(Array.isArray(next) ? next : []);
          setError("");
        })
        .catch(() => setError("The queue is unavailable right now.")),
    [],
  );

  // Live updates arrive over the event stream; the poll is a safety net for a
  // dropped connection.
  useEffect(() => {
    void refresh();
    const poll = window.setInterval(refresh, pollIntervalMs);
    const close = connectToJobEvents({
      onUpdate: refresh,
      onReconnect: refresh,
      reconnectDelaysMs,
    });
    return () => {
      window.clearInterval(poll);
      close();
    };
  }, [pollIntervalMs, reconnectDelaysMs, refresh]);

  return (
    <section className="queue" aria-label="Download queue">
      <h2>
        Downloads{" "}
        <span className="row-count">
          {jobs.length} {jobs.length === 1 ? "title" : "titles"}
        </span>
      </h2>
      {error && (
        <p className="error-note" role="alert">
          {error}
        </p>
      )}
      {jobs.length === 0 && !error ? (
        <p className="empty">Nothing queued yet. Pick something from Browse to get started.</p>
      ) : (
        jobs.map((job) => <JobCard key={job.id} job={job} onChanged={() => void refresh()} />)
      )}
    </section>
  );
}
