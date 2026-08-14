import type { ApiRequestInit, ErrorEnvelope, Job, Session } from "./types";

let csrfToken = "";
let onUnauthorized: (() => void) | undefined;

export function setUnauthorizedHandler(handler: () => void) { onUnauthorized = handler; }

async function readError(response: Response): Promise<Error> {
  const body = await response.json().catch(() => null) as ErrorEnvelope | null;
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
    if (response.status === 401 && init.retryOnAuth !== false) { onUnauthorized?.(); }
    if (!response.ok) throw await readError(response);
    if (response.status === 204) return undefined as T;
    return await response.json() as T;
  },
  async session(): Promise<Session> {
    const session = await this.request<Session>("/auth/session", { retryOnAuth: false });
    csrfToken = session.csrf_token;
    return session;
  },
  jobs: {
    list: () => api.request<Job[]>("/jobs"),
    action: (id: string, action: "pause" | "resume" | "cancel" | "retry", version: number) => api.request<Job>(`/jobs/${encodeURIComponent(id)}/${action}`, { method: "POST", body: JSON.stringify({ version }) }),
  },
};
