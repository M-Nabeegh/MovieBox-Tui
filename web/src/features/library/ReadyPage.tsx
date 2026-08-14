import type { Job } from "../../api/types";

export function ReadyPage({ job }: { job: Job }) {
  return <article className="ready-card" aria-label={`${job.title} ready`}>
    <div className="ready-heading"><div><p className="eyebrow">READY</p><h2>{job.title}</h2></div><span className="state-pill ready">Ready</span></div>
    <p className="muted">Jellyfin will play this from your media library when the integration is enabled.</p>
    {job.warning && <p className="warning" role="status">{job.warning}</p>}
    {job.updated_at && <p className="muted">Completed {new Date(job.updated_at).toLocaleString()}</p>}
    <a className="quiet-button ready-link" href="#" onClick={(event) => event.preventDefault()} aria-label="Open in Jellyfin">Open in Jellyfin</a>
    <p className="muted ready-placeholder">Jellyfin link will be available in Task 12.</p>
  </article>;
}
