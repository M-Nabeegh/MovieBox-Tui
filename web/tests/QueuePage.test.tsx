import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueuePage } from "../src/features/queue/QueuePage";

const jobs = [
  { id: "queued-1", title: "Queued Movie", year: "2024", media_type: "movie", state: "queued", requested_height: 1080, downloaded_bytes: 0, total_bytes: null, speed_bytes_per_second: null, attempt: 0, error_code: null, error_message: null, warning: null, version: 1 },
  { id: "downloading-1", title: "Downloading Movie", year: "2023", media_type: "movie", state: "downloading", requested_height: 720, downloaded_bytes: 50_000_000, total_bytes: 100_000_000, speed_bytes_per_second: 10_000_000, attempt: 1, error_code: null, error_message: null, warning: null, version: 2 },
  { id: "paused-1", title: "Paused Movie", year: "2022", media_type: "movie", state: "paused", requested_height: 720, downloaded_bytes: 10, total_bytes: null, speed_bytes_per_second: null, attempt: 1, error_code: null, error_message: null, warning: null, version: 3 },
  { id: "failed-1", title: "Failed Movie", year: "2021", media_type: "movie", state: "failed", requested_height: 1080, downloaded_bytes: 10, total_bytes: null, speed_bytes_per_second: null, attempt: 2, error_code: "provider_unavailable", error_message: "The provider is unavailable.", warning: "Subtitle could not be saved.", version: 4 },
  { id: "ready-1", title: "Ready Movie", year: "2020", media_type: "movie", state: "ready", requested_height: 1080, downloaded_bytes: 100, total_bytes: 100, speed_bytes_per_second: null, attempt: 1, error_code: null, error_message: null, warning: "Subtitle could not be saved.", version: 5 },
];

function response(body: unknown, status = 200) {
  return new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
}

test("renders safe job states and progress with legal controls", async () => {
  const user = userEvent.setup();
  const fetchMock = vi.spyOn(globalThis, "fetch").mockImplementation(async () => response(jobs));
  render(<QueuePage />);

  expect(await screen.findByText("Downloading Movie")).toBeVisible();
  expect(screen.getByText("50.0 MB of 100.0 MB")).toBeVisible();
  expect(screen.getByText("5s remaining")).toBeVisible();
  expect(screen.getByRole("button", { name: "Pause Downloading Movie" })).toBeVisible();
  expect(screen.getByRole("button", { name: "Resume Paused Movie" })).toBeVisible();
  expect(screen.getByRole("button", { name: "Retry Failed Movie" })).toBeVisible();
  expect(screen.queryByRole("button", { name: "Pause Queued Movie" })).not.toBeInTheDocument();

  await user.click(screen.getByRole("button", { name: "Cancel Downloading Movie" }));
  expect(screen.getByRole("alertdialog")).toHaveTextContent(/cancel/i);
  await user.click(screen.getByRole("button", { name: "Confirm cancel" }));
  await waitFor(() => expect(fetchMock).toHaveBeenCalledWith("/api/jobs/downloading-1/cancel", expect.objectContaining({ method: "POST" })));

  expect(screen.getByText(/Jellyfin link will be available in Task 12/i)).toBeVisible();
  expect(screen.getAllByText(/subtitle could not be saved/i).length).toBeGreaterThan(0);
  fetchMock.mockRestore();
});

test("refreshes jobs after a job.updated event and reconnects with capped backoff", async () => {
  const sources: Array<{ onUpdate?: (event: MessageEvent) => void; onerror?: (event: Event) => void; close: () => void }> = [];
  class FakeEventSource {
    onerror?: (event: Event) => void;
    addEventListener(_name: string, handler: (event: MessageEvent) => void) { this.onUpdate = handler; }
    onUpdate?: (event: MessageEvent) => void;
    close = vi.fn();
    constructor() { sources.push(this); }
  }
  vi.stubGlobal("EventSource", FakeEventSource);
  const fetchMock = vi.spyOn(globalThis, "fetch").mockResolvedValue(response([]));
  render(<QueuePage pollIntervalMs={60_000} reconnectDelaysMs={[0]} />);
  await waitFor(() => expect(fetchMock).toHaveBeenCalledWith("/api/jobs", expect.anything()));
  const initialCalls = fetchMock.mock.calls.length;
  sources[0].onUpdate?.(new MessageEvent("job.updated", { data: JSON.stringify({ job_id: "queued-1", state: "downloading", version: 2 }) }));
  await waitFor(() => expect(fetchMock.mock.calls.length).toBeGreaterThan(initialCalls));
  sources[0].onerror?.(new Event("error"));
  await waitFor(() => expect(sources.length).toBeGreaterThan(1));
  fetchMock.mockRestore();
  vi.unstubAllGlobals();
});
