import { useState } from "react";
import { api } from "../../api/client";
import type { Job, JobState } from "../../api/types";

const LABELS: Record<JobState, string> = {
  queued: "Queued",
  resolving: "Resolving",
  downloading: "Downloading",
  paused: "Paused",
  finalizing: "Finalizing",
  ready: "Ready to watch",
  failed: "Failed",
  cancelled: "Cancelled",
};

function size(value: number | null) {
  if (value === null) return null;
  const gb = value / 1024 ** 3;
  return gb >= 1 ? `${gb.toFixed(1)} GB` : `${Math.round(value / 1024 ** 2)} MB`;
}

function remaining(job: Job) {
  if (!job.total_bytes || !job.speed_bytes_per_second) return null;
  const seconds = Math.ceil((job.total_bytes - job.downloaded_bytes) / job.speed_bytes_per_second);
  if (seconds < 60) return `${seconds}s left`;
  return `${Math.ceil(seconds / 60)} min left`;
}

export function JobCard({ job, onChanged }: { job: Job; onChanged: () => void }) {
  const [working, setWorking] = useState(false);
  const [confirming, setConfirming] = useState(false);

  const action =
    job.state === "downloading"
      ? "pause"
      : job.state === "paused"
        ? "resume"
        : job.state === "failed"
          ? "retry"
          : null;
  const cancellable = ["queued", "resolving", "downloading", "paused"].includes(job.state);
  const percent =
    job.total_bytes && job.total_bytes > 0
      ? Math.min(100, (job.downloaded_bytes / job.total_bytes) * 100)
      : null;

  async function run(next: "pause" | "resume" | "cancel" | "retry") {
    setWorking(true);
    try {
      await api.jobs.action(job.id, next, job.version);
      onChanged();
    } finally {
      setWorking(false);
      setConfirming(false);
    }
  }

  return (
    <article className="job" aria-label={`${job.title}: ${LABELS[job.state]}`}>
      <div className="job-head">
        <span className="job-title">{job.title}</span>
        <span className="card-meta">
          {job.year ?? "—"} · {job.requested_height}p
        </span>
        <span className="job-state">{LABELS[job.state]}</span>
      </div>

      {percent !== null && (
        <div className="progress">
          <span style={{ width: `${percent}%` }} />
        </div>
      )}

      {job.total_bytes !== null && (
        <p className="card-meta">
          {size(job.downloaded_bytes)} of {size(job.total_bytes)}
          {job.speed_bytes_per_second ? ` · ${size(job.speed_bytes_per_second)}/s` : ""}
          {remaining(job) ? ` · ${remaining(job)}` : ""}
        </p>
      )}

      {job.error_message && (
        <p className="error-note" role="alert">
          {job.error_message}
        </p>
      )}
      {job.warning && (
        <p className="card-meta" role="status">
          {job.warning}
        </p>
      )}

      {(action || cancellable) && (
        <div className="job-actions">
          {action && (
            <button className="btn btn-ghost" disabled={working} onClick={() => run(action)}>
              {action === "pause" ? "Pause" : action === "resume" ? "Resume" : "Retry"}
            </button>
          )}
          {cancellable && !confirming && (
            <button className="btn btn-quiet" disabled={working} onClick={() => setConfirming(true)}>
              Cancel
            </button>
          )}
          {confirming && (
            <>
              <button className="btn btn-quiet" onClick={() => setConfirming(false)}>
                Keep it
              </button>
              <button className="btn btn-primary" onClick={() => run("cancel")}>
                Confirm cancel
              </button>
            </>
          )}
        </div>
      )}
    </article>
  );
}
