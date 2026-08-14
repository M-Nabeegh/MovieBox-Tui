import type { SourceOption } from "../../api/types";

export function SourcePicker({ sources, value, onChange }: { sources: SourceOption[]; value: string; onChange: (id: string) => void }) {
  return <fieldset><legend>Video source</legend>{sources.filter((source) => source.height <= 1080).map((source) => <label className="choice" key={source.id}><input aria-label={source.label} type="radio" name="source" value={source.id} checked={source.id === value} onChange={() => onChange(source.id)} /> <span><strong>{source.label}</strong>{source.recommended && <em>Recommended</em>}<small>{source.size_bytes ? `${(source.size_bytes / 1_000_000_000).toFixed(1)} GB` : "Size unavailable"}</small></span></label>)}</fieldset>;
}
