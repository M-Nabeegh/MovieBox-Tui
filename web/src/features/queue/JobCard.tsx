import { useState } from "react";
import { api } from "../../api/client";
import type { Job, JobState } from "../../api/types";
import { ReadyPage } from "../library/ReadyPage";

const labels: Record<JobState, string> = { queued: "Queued", resolving: "Resolving", downloading: "Downloading", paused: "Paused", finalizing: "Finalizing", ready: "Ready", failed: "Failed", cancelled: "Cancelled" };
function bytes(value: number | null) { if (value === null) return null; if (value < 1_000_000) return `${value} B`; return `${(value / (value >= 1_000_000_000 ? 1_000_000_000 : 1_000_000)).toFixed(1)} ${value >= 1_000_000_000 ? "GB" : "MB"}`; }
function eta(job: Job) { if (!job.total_bytes || !job.speed_bytes_per_second) return null; return `${Math.ceil((job.total_bytes - job.downloaded_bytes) / job.speed_bytes_per_second)}s remaining`; }

export function JobCard({ job, onChanged }: { job: Job; onChanged: () => void }) {
  const [confirming, setConfirming] = useState(false);
  const [working, setWorking] = useState(false);
  if (job.state === "ready") return <ReadyPage job={job} />;
  const action = job.state === "downloading" ? "pause" : job.state === "paused" ? "resume" : job.state === "failed" ? "retry" : null;
  async function run(next: "pause" | "resume" | "cancel" | "retry") { setWorking(true); try { await api.jobs.action(job.id, next, job.version); onChanged(); } finally { setWorking(false); setConfirming(false); } }
  return <article className="job-card" aria-label={`${job.title} ${labels[job.state]}`}>
    <div className="job-heading"><div><p className="eyebrow">DOWNLOAD</p><h2>{job.title}</h2><p className="muted">{job.year ?? "Year unknown"} · {job.requested_height}p</p></div><span className={`state-pill ${job.state}`}>{labels[job.state]}</span></div>
    {job.total_bytes !== null && <div className="progress-track" aria-label={`${job.title} progress`}><span style={{ width: `${Math.min(100, (job.downloaded_bytes / job.total_bytes) * 100)}%` }} /></div>}
    {job.total_bytes !== null && <p className="progress-copy"><span>{bytes(job.downloaded_bytes)} of {bytes(job.total_bytes)}</span>{job.speed_bytes_per_second ? <><span aria-hidden="true"> · </span><span>{bytes(job.speed_bytes_per_second)}/s</span></> : null}{eta(job) ? <><span aria-hidden="true"> · </span><span>{eta(job)}</span></> : null}</p>}
    {job.error_message && <p className="form-error" role="alert">{job.error_message} <span className="request-id">Request ID: {job.id}</span></p>}
    {job.warning && <p className="warning" role="status">{job.warning}</p>}
    <div className="job-actions">
      {action && <button className="quiet-button" disabled={working} onClick={() => run(action)}>{action === "pause" ? `Pause ${job.title}` : action === "resume" ? `Resume ${job.title}` : `Retry ${job.title}`}</button>}
      {(job.state === "queued" || job.state === "resolving" || job.state === "downloading" || job.state === "paused") && <button className="quiet-button danger-button" disabled={working} onClick={() => setConfirming(true)}>Cancel {job.title}</button>}
    </div>
    {confirming && <div className="confirmation" role="alertdialog" aria-label="Confirm cancel"><strong>Cancel this download?</strong><p>The partial download will no longer continue.</p><div className="confirmation-actions"><button className="quiet-button" onClick={() => setConfirming(false)}>Keep download</button><button className="primary-action" onClick={() => run("cancel")}>Confirm cancel</button></div></div>}
  </article>;
}
