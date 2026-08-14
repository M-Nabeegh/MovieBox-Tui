import { useCallback, useEffect, useState } from "react";
import { connectToJobEvents } from "../../api/events";
import { api } from "../../api/client";
import type { Job } from "../../api/types";
import { JobCard } from "./JobCard";

export function QueuePage({ pollIntervalMs = 30_000, reconnectDelaysMs }: { pollIntervalMs?: number; reconnectDelaysMs?: number[] } = {}) {
  const [jobs, setJobs] = useState<Job[]>([]);
  const [error, setError] = useState("");
  const refresh = useCallback(() => api.jobs.list().then((next) => { setJobs(Array.isArray(next) ? next : []); setError(""); }).catch(() => setError("Queue is unavailable right now.")), []);
  useEffect(() => { void refresh(); const poll = window.setInterval(refresh, pollIntervalMs); const close = connectToJobEvents({ onUpdate: refresh, onReconnect: refresh, reconnectDelaysMs }); return () => { window.clearInterval(poll); close(); }; }, [pollIntervalMs, reconnectDelaysMs, refresh]);
  return <section className="queue-panel" aria-label="Download queue"><div className="results-heading"><div><p className="eyebrow">QUEUE</p><h2>Your downloads</h2></div><span className="muted">{jobs.length} {jobs.length === 1 ? "title" : "titles"}</span></div>{error && <p className="form-error" role="alert">{error}</p>}{jobs.length === 0 && !error ? <p className="muted queue-empty">Nothing is queued yet. Choose a title to start a download.</p> : <div className="queue-list">{jobs.map((job) => <JobCard key={job.id} job={job} onChanged={() => void refresh()} />)}</div>}</section>;
}
