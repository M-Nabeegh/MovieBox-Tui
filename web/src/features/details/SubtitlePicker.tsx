import type { SubtitleTrack } from "../../api/types";

export function SubtitlePicker({ subtitles, value, onChange }: { subtitles: SubtitleTrack[]; value: string; onChange: (id: string) => void }) {
  return <label className="select-label">Subtitles<select aria-label="Subtitles" value={value} onChange={(event) => onChange(event.target.value)}><option value="">None</option>{subtitles.map((subtitle) => <option key={subtitle.id} value={subtitle.id}>{subtitle.language}{subtitle.format ? ` · ${subtitle.format.toUpperCase()}` : ""}</option>)}</select></label>;
}
