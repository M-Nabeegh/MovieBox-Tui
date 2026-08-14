import type { CatalogItem } from "../../api/types";

export function ResultCard({ item, onSelect }: { item: CatalogItem; onSelect: (item: CatalogItem) => void }) {
  return <button className="result-card" type="button" onClick={() => onSelect(item)} aria-label={item.title}>
    <span className="poster-fallback" aria-hidden="true">{item.title.slice(0, 1).toUpperCase()}</span>
    <span><strong>{item.title}</strong><small>{item.year ?? "Year unknown"} · {item.media_type === "series" ? "Series" : "Movie"}</small></span>
  </button>;
}
