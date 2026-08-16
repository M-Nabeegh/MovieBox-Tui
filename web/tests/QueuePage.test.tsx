import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueuePage } from "../src/features/queue/QueuePage";

const jobs = [
  {
    id: "downloading-1",
    title: "Downloading Movie",
    year: "2023",
    media_type: "movie",
    state: "downloading",
    requested_height: 720,
    downloaded_bytes: 52_428_800,
    total_bytes: 104_857_600,
    speed_bytes_per_second: 10_485_760,
    attempt: 1,
    error_code: null,
    error_message: null,
    warning: null,
    version: 2,
  },
  {
    id: "paused-1",
    title: "Paused Movie",
    year: "2022",
    media_type: "movie",
    state: "paused",
    requested_height: 720,
    downloaded_bytes: 10,
    total_bytes: null,
    speed_bytes_per_second: null,
    attempt: 1,
    error_code: null,
    error_message: null,
    warning: null,
    version: 3,
  },
  {
    id: "failed-1",
    title: "Failed Movie",
    year: "2021",
    media_type: "movie",
    state: "failed",
    requested_height: 1080,
    downloaded_bytes: 10,
    total_bytes: null,
    speed_bytes_per_second: null,
    attempt: 2,
    error_code: "provider_unavailable",
    error_message: "The provider is unavailable.",
    warning: null,
    version: 4,
  },
];

function json(body: unknown, status = 200) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

test("shows progress and the control that matches each job state", async () => {
  const fetchMock = vi.spyOn(globalThis, "fetch").mockImplementation(async () => json(jobs));
  render(<QueuePage />);

  expect(await screen.findByText("Downloading Movie")).toBeVisible();
  expect(screen.getByText(/50 MB of 100 MB/)).toBeVisible();

  // Each state offers exactly the action that applies to it.
  expect(screen.getByRole("button", { name: "Pause" })).toBeVisible();
  expect(screen.getByRole("button", { name: "Resume" })).toBeVisible();
  expect(screen.getByRole("button", { name: "Retry" })).toBeVisible();
  expect(screen.getByRole("alert")).toHaveTextContent("The provider is unavailable.");

  fetchMock.mockRestore();
});

test("cancelling asks for confirmation before sending the request", async () => {
  const user = userEvent.setup();
  const calls: string[] = [];
  const fetchMock = vi.spyOn(globalThis, "fetch").mockImplementation(async (input, init) => {
    const url = String(input);
    if ((init?.method ?? "GET") === "POST") {
      calls.push(url);
      return json(jobs[0]);
    }
    return json(jobs);
  });

  render(<QueuePage />);
  await screen.findByText("Downloading Movie");

  await user.click(screen.getAllByRole("button", { name: "Cancel" })[0]);
  // Nothing is sent until the destructive action is confirmed.
  expect(calls).toHaveLength(0);

  await user.click(screen.getByRole("button", { name: /confirm cancel/i }));
  await waitFor(() => expect(calls.some((url) => url.includes("/cancel"))).toBe(true));

  fetchMock.mockRestore();
});

test("an unavailable queue reports itself instead of rendering empty", async () => {
  const fetchMock = vi
    .spyOn(globalThis, "fetch")
    .mockImplementation(async () => new Response(null, { status: 500 }));

  render(<QueuePage />);
  expect(await screen.findByRole("alert")).toHaveTextContent(/unavailable/i);
  fetchMock.mockRestore();
});
