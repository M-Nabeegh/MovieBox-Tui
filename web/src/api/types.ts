export type ErrorEnvelope = { error: { code: string; message: string; request_id: string; fields: Record<string, string> } };
export type Session = { authenticated: boolean; username: string; display_name: string; csrf_token: string };
export type ApiRequestInit = RequestInit & { retryOnAuth?: boolean };
export type MediaType = "movie" | "series";
export type CatalogItem = { id: string; title: string; year: string | null; media_type: MediaType; season_count: number | null };
export type SearchPage = { page: number; items: CatalogItem[]; has_more: boolean };
export type EpisodeInfo = { number: number; title: string | null };
export type SeasonInfo = { number: number; episodes: EpisodeInfo[] };
export type CatalogDetails = { id: string; title: string; media_type: MediaType; year: string | null; description: string | null; tagline: string | null; imdb_rating: string | null; duration: string | null; genres: string[]; country: string | null; seasons: SeasonInfo[]; audio_options: { id: string; label: string; original: boolean }[] };
export type SourceOption = { id: string; height: number; label: string; size_bytes: number | null; language: string | null; recommended: boolean; quality: QualityTier };
export type SubtitleTrack = { id: string; language: string; format: string | null };
export type CreateJobRequest = { catalog_id: string; source_id: string; subtitle_id: string | null; requested_height: number; season?: number; episode?: number };
export type JobState = "queued" | "resolving" | "downloading" | "paused" | "finalizing" | "ready" | "failed" | "cancelled";
export type Job = { id: string; catalog_id: string; source_id: string; subtitle_id: string | null; title: string; year: string | null; media_type: MediaType; season: number | null; episode: number | null; episode_title: string | null; requested_height: number; state: JobState; downloaded_bytes: number; total_bytes: number | null; speed_bytes_per_second: number | null; attempt: number; error_code: string | null; error_message: string | null; warning: string | null; version: number; updated_at?: string | null };
export type JobUpdatedEvent = Pick<Job, "id" | "state" | "downloaded_bytes" | "total_bytes" | "speed_bytes_per_second" | "error_code" | "error_message" | "warning"> & { kind?: string; version?: number };

/** How a source compares with the others offered for the same title. */
export type QualityTier = "best" | "good" | "lower";

/** A title as the metadata catalogue describes it, with artwork for browsing. */
export type MediaKind = "movie" | "series";

export type DiscoverTitle = {
  tmdb_id: number;
  title: string;
  year: string | null;
  overview: string | null;
  poster_url: string | null;
  backdrop_url: string | null;
  rating: number | null;
  language: string | null;
  media_kind: MediaKind;
};

/** A named strip of titles in the browse view. */
export type DiscoverRow = { id: string; title: string; items: DiscoverTitle[] };

/** Whether a finished download has been indexed by the media server yet. */
export type LibraryStatus = {
  status: "ready" | "scan_pending" | "unavailable" | string;
  url: string | null;
};
