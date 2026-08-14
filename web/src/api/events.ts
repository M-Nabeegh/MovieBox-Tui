import type { JobUpdatedEvent } from "./types";

type EventSourceLike = { addEventListener: (name: string, listener: (event: MessageEvent) => void) => void; close: () => void; onerror: ((event: Event) => void) | null };
type EventSourceFactory = (url: string) => EventSourceLike;

const defaultDelays = [1000, 2000, 4000, 8000, 15000];

export function connectToJobEvents(options: { onUpdate: (event: JobUpdatedEvent) => void; onReconnect?: () => void; reconnectDelaysMs?: number[]; eventSourceFactory?: EventSourceFactory }) {
  const delays = options.reconnectDelaysMs ?? defaultDelays;
  if (!options.eventSourceFactory && typeof EventSource === "undefined") return () => undefined;
  const makeSource = options.eventSourceFactory ?? ((url) => new EventSource(url));
  let source: EventSourceLike | null = null;
  let timer: number | undefined;
  let attempt = 0;
  let closed = false;

  function connect() {
    if (closed) return;
    const nextSource = makeSource("/api/events");
    source = nextSource;
    nextSource.addEventListener("job.updated", (event) => {
      try { options.onUpdate(JSON.parse(event.data) as JobUpdatedEvent); } catch { /* Ignore malformed server events and rely on polling. */ }
    });
    nextSource.onerror = () => {
      nextSource.close();
      source = null;
      if (closed) return;
      const delay = delays[Math.min(attempt, delays.length - 1)] ?? 15000;
      attempt += 1;
      timer = window.setTimeout(() => { options.onReconnect?.(); connect(); }, delay);
    };
  }

  connect();
  return () => { closed = true; if (timer !== undefined) window.clearTimeout(timer); source?.close(); source = null; };
}
