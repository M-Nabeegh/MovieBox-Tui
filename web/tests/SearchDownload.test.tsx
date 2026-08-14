import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import App from "../src/App";

const session = { authenticated: true, username: "admin", csrf_token: "csrf-fixture" };
const searchResult = { page: 1, has_more: false, items: [{ id: "movie-1", title: "Dune", year: "2021", media_type: "movie", season_count: null }] };
const details = { id: "movie-1", title: "Dune", media_type: "movie", year: "2021", description: "A desert epic.", tagline: null, imdb_rating: "8.0", duration: "2h 35m", genres: ["Sci-Fi"], country: null, seasons: [], audio_options: [] };
const sources = [
  { id: "source-1080", height: 1080, label: "1080p", size_bytes: 4_000_000_000, language: "English", recommended: true },
  { id: "source-720", height: 720, label: "720p", size_bytes: 2_000_000_000, language: "English", recommended: false },
  { id: "source-2160", height: 2160, label: "2160p", size_bytes: 8_000_000_000, language: "English", recommended: false },
];
const subtitles = [{ id: "sub-en", language: "English", format: "srt" }];

function response(body: unknown, status = 200) {
  return new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
}

test("debounces search, opens details, and confirms a capped download", async () => {
  const user = userEvent.setup();
  const fetchMock = vi.spyOn(globalThis, "fetch").mockImplementation(async (input, init) => {
    const url = String(input);
    if (url.endsWith("/auth/session")) return response(session);
    if (url.includes("/catalog/search")) return response(searchResult);
    if (url.endsWith("/catalog/items/moviebox/movie-1")) return response(details);
    if (url.includes("/sources")) return response(sources);
    if (url.includes("/subtitles")) return response(subtitles);
    if (url.endsWith("/jobs") && init?.method === "POST") return response({ id: "job-1", state: "queued" }, 202);
    throw new Error(`Unexpected request: ${url}`);
  });

  render(<App />);
  const input = await screen.findByLabelText(/find a film or series/i);
  await user.type(input, "Dune");
  expect(fetchMock).not.toHaveBeenCalledWith(expect.stringContaining("/catalog/search"), expect.anything());
  await waitFor(() => expect(fetchMock).toHaveBeenCalledWith(expect.stringContaining("/catalog/search"), expect.anything()), { timeout: 1000 });
  await waitFor(() => expect(screen.getByText("Dune")).toBeVisible());

  await user.click(screen.getByRole("button", { name: /dune/i }));
  expect(await screen.findByRole("dialog")).toBeVisible();
  expect(screen.getByLabelText("1080p")).toBeChecked();
  expect(screen.queryByLabelText("2160p")).not.toBeInTheDocument();
  expect(screen.getByLabelText(/subtitles/i)).toHaveValue("sub-en");

  await user.click(screen.getByRole("button", { name: /download/i }));
  expect(await screen.findByText(/confirm download/i)).toBeVisible();
  await user.click(screen.getByRole("button", { name: /confirm/i }));
  await waitFor(() => expect(fetchMock).toHaveBeenCalledWith("/api/jobs", expect.objectContaining({ method: "POST" })));
  const jobRequest = fetchMock.mock.calls.find(([url, init]) => String(url).endsWith("/api/jobs") && init?.method === "POST");
  expect(JSON.parse(String(jobRequest?.[1]?.body))).toMatchObject({ catalog_id: "movie-1", source_id: "source-1080", requested_height: 1080, subtitle_id: "sub-en" });

  fetchMock.mockRestore();
});
