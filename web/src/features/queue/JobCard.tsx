import { useState } from "react";
import { api } from "../../api/client";
import type { Job, JobState } from "../../api/types";
import { ReadyCard } from "./ReadyCard";

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

function isActivelyTransferring(state: JobState) {
  return ["resolving", "downloading", "finalizing"].includes(state);
}

export function JobCard({ job, onChanged }: { job: Job; onChanged: () => void }) {
  const [working, setWorking] = useState(false);
  const [confirming, setConfirming] = useState(false);

  // A finished download is a different thing from one in progress: it has no
  // controls, only somewhere to go and watch it.
  if (job.state === "ready") return <ReadyCard job={job} />;

  const action =
    job.state === "downloading"
      ? "pause"
      : job.state === "paused"
        ? "resume"
        : job.state === "failed"
          ? "retry"
          : null;
  const cancellable = ["queued", "resolving", "downloading", "paused"].includes(job.state);
  // A job that can no longer run is only clutter. Removing it clears the
  // record; anything already in the library stays where it is.
  const removable = ["failed", "cancelled"].includes(job.state);
  const percent =
    job.total_bytes && job.total_bytes > 0
      ? Math.min(100, (job.downloaded_bytes / job.total_bytes) * 100)
      : null;
  const indeterminate = percent === null && isActivelyTransferring(job.state);
  const hasProgress = percent !== null || indeterminate;
  const hasTransferStats =
    job.total_bytes !== null || job.downloaded_bytes > 0 || job.speed_bytes_per_second !== null;

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

  async function remove() {
    setWorking(true);
    try {
      await api.jobs.remove(job.id);
      onChanged();
    } finally {
      setWorking(false);
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

      {hasProgress && (
        <div
          className={`progress${indeterminate ? " progress-indeterminate" : ""}`}
          role="progressbar"
          aria-label="Download progress"
          aria-valuemin={0}
          aria-valuemax={percent !== null ? 100 : undefined}
          aria-valuenow={percent !== null ? Math.round(percent) : undefined}
          aria-valuetext={percent !== null ? `${Math.round(percent)}% downloaded` : "Download size calculating"}
        >
          <span style={percent !== null ? { width: `${percent}%` } : undefined} />
        </div>
      )}

      {hasTransferStats && (
        <p className="card-meta">
          {job.total_bytes !== null
            ? `${size(job.downloaded_bytes)} of ${size(job.total_bytes)}`
            : `${size(job.downloaded_bytes)} downloaded`}
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

      {(action || cancellable || removable) && (
        <div className="job-actions">
          {action && (
            <button className="btn btn-ghost" disabled={working} onClick={() => run(action)}>
              {action === "pause" ? "Pause" : action === "resume" ? "Resume" : "Retry"}
            </button>
          )}
          {removable && (
            <button className="btn btn-quiet" disabled={working} onClick={remove}>
              Remove
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
