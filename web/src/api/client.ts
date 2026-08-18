import type {
  CatalogDetails,
  ApiRequestInit,
  CreateJobRequest,
  DiscoverRow,
  DiscoverTitle,
  ErrorEnvelope,
  Job,
  LibraryStatus,
  SearchPage,
  Session,
  SourceOption,
} from "./types";

let csrfToken = "";
let onUnauthorized: (() => void) | undefined;

export function setUnauthorizedHandler(handler: () => void) {
  onUnauthorized = handler;
}

async function readError(response: Response): Promise<Error> {
  const body = (await response.json().catch(() => null)) as ErrorEnvelope | null;
  return new Error(body?.error?.message ?? `Request failed (${response.status})`);
}

export const api = {
  async request<T>(path: string, init: ApiRequestInit = {}): Promise<T> {
    const method = (init.method ?? "GET").toUpperCase();
    const headers = new Headers(init.headers);
    headers.set("Accept", "application/json");
    if (init.body && !headers.has("Content-Type")) headers.set("Content-Type", "application/json");
    if (method !== "GET" && method !== "HEAD" && csrfToken) headers.set("X-CSRF-Token", csrfToken);
    const response = await fetch(`/api${path}`, { ...init, headers, credentials: "same-origin" });
    if (response.status === 401 && init.retryOnAuth !== false) onUnauthorized?.();
    if (!response.ok) throw await readError(response);
    if (response.status === 204) return undefined as T;
    return (await response.json()) as T;
  },

  async session(): Promise<Session> {
    const session = await this.request<Session>("/auth/session", { retryOnAuth: false });
    csrfToken = session.csrf_token;
    return session;
  },

  discover: {
    rows: () => api.request<DiscoverRow[]>("/discover"),
    search: (query: string) =>
      api.request<DiscoverTitle[]>(`/discover/search?q=${encodeURIComponent(query)}`),
    /** Titles narrowed to one language, and optionally one year. */
    filter: (options: { language: string; year: number | null; kind: "movie" | "series" }) => {
      const params = new URLSearchParams({ language: options.language, kind: options.kind });
      if (options.year !== null) params.set("year", String(options.year));
      return api.request<DiscoverTitle[]>(`/discover/filter?${params}`);
    },
  },

  catalog: {
    search: (query: string) =>
      api.request<SearchPage>(`/catalog/search?q=${encodeURIComponent(query)}`),
    /** Sources for a film, or for one episode when season and episode are given. */
    sources: (catalogId: string, episode?: { season: number; episode: number }) => {
      const query = episode ? `?season=${episode.season}&episode=${episode.episode}` : "";
      return api.request<SourceOption[]>(
        `/catalog/items/moviebox/${encodeURIComponent(catalogId)}/sources${query}`,
      );
    },
    details: (catalogId: string) =>
      api.request<CatalogDetails>(`/catalog/items/moviebox/${encodeURIComponent(catalogId)}`),
  },

  library: {
    forJob: (jobId: string) =>
      api.request<LibraryStatus>(`/library/${encodeURIComponent(jobId)}`),
  },

  createJob: (body: CreateJobRequest) =>
    api.request<Job>("/jobs", { method: "POST", body: JSON.stringify(body) }),

  jobs: {
    list: () => api.request<Job[]>("/jobs"),
    action: (id: string, action: "pause" | "resume" | "cancel" | "retry", version: number) =>
      api.request<Job>(`/jobs/${encodeURIComponent(id)}/${action}`, {
        method: "POST",
        body: JSON.stringify({ version }),
      }),
  },
};
