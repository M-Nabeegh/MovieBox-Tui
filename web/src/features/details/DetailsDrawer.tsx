import { useEffect, useMemo, useState } from "react";
import { api } from "../../api/client";
import type { CatalogDetails, CatalogItem, CreateJobRequest, SourceOption, SubtitleTrack } from "../../api/types";
import { SourcePicker } from "./SourcePicker";
import { SubtitlePicker } from "./SubtitlePicker";

function size(bytes: number | null) { return bytes ? `${(bytes / 1_000_000_000).toFixed(1)} GB` : "size unavailable"; }

export function DetailsDrawer({ item, onClose, onQueued }: { item: CatalogItem; onClose: () => void; onQueued: () => void }) {
  const [details, setDetails] = useState<CatalogDetails | null>(null);
  const [sources, setSources] = useState<SourceOption[]>([]);
  const [subtitles, setSubtitles] = useState<SubtitleTrack[]>([]);
  const [sourceId, setSourceId] = useState("");
  const [subtitleId, setSubtitleId] = useState("");
  const [confirming, setConfirming] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState("");
  const source = useMemo(() => sources.find((candidate) => candidate.id === sourceId), [sources, sourceId]);

  useEffect(() => {
    let active = true;
    Promise.all([
      api.request<CatalogDetails>(`/catalog/items/moviebox/${encodeURIComponent(item.id)}`),
      api.request<SourceOption[]>(`/catalog/items/moviebox/${encodeURIComponent(item.id)}/sources`),
    ]).then(([loadedDetails, loadedSources]) => {
      if (!active) return;
      const safeSources = loadedSources.filter((candidate) => candidate.height <= 1080);
      setDetails(loadedDetails); setSources(safeSources); setSourceId((safeSources.find((candidate) => candidate.recommended) ?? safeSources[0])?.id ?? "");
      const preferredSource = safeSources.find((candidate) => candidate.recommended) ?? safeSources[0];
      if (preferredSource) return api.request<SubtitleTrack[]>(`/catalog/items/moviebox/${encodeURIComponent(item.id)}/subtitles?source_id=${encodeURIComponent(preferredSource.id)}`);
      return [];
    }).then((loadedSubtitles) => { if (active && Array.isArray(loadedSubtitles)) { setSubtitles(loadedSubtitles); setSubtitleId(loadedSubtitles[0]?.id ?? ""); } }).catch(() => { if (active) setError("Details are unavailable right now."); });
    return () => { active = false; };
  }, [item.id]);

  async function createJob() {
    if (!source) return;
    setSubmitting(true); setError("");
    const request: CreateJobRequest = { catalog_id: item.id, source_id: source.id, subtitle_id: subtitleId || null, requested_height: source.height };
    try { await api.request("/jobs", { method: "POST", body: JSON.stringify(request) }); onQueued(); onClose(); } catch { setError("Could not queue this download."); setSubmitting(false); }
  }

  return <div className="drawer-backdrop"><section className="details-drawer" role="dialog" aria-label={`${item.title} details`}><button className="drawer-close" onClick={onClose} aria-label="Close details">×</button>
    {!details && !error && <p className="muted">Loading details…</p>}
    {error && <p className="form-error" role="alert">{error}</p>}
    {details && <><p className="eyebrow">{details.media_type === "series" ? "SERIES" : "MOVIE"} · {details.year ?? "YEAR UNKNOWN"}</p><h2>{details.title}</h2><p className="muted">{details.description ?? "No description available."}</p><SourcePicker sources={sources} value={sourceId} onChange={setSourceId} /><SubtitlePicker subtitles={subtitles} value={subtitleId} onChange={setSubtitleId} />
      <button className="primary-action" disabled={!source || submitting} onClick={() => setConfirming(true)}>Download</button>
      {confirming && source && <div className="confirmation" role="alertdialog" aria-label="Confirm download"><strong>Confirm download</strong><p>{details.media_type === "series" ? "Episode" : "Movie"}: {details.title}<br />Quality: {source.height}p · Subtitle: {subtitleId ? subtitles.find((subtitle) => subtitle.id === subtitleId)?.language : "None"}<br />Estimated size: {size(source.size_bytes)}</p><div className="confirmation-actions"><button className="quiet-button" disabled={submitting} onClick={() => setConfirming(false)}>Cancel</button><button className="primary-action" disabled={submitting} onClick={createJob}>{submitting ? "Queueing…" : "Confirm"}</button></div></div>}
    </>}
  </section></div>;
}
