import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import App from "../src/App";

function json(body: unknown, status = 200) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

const session = { authenticated: true, username: "admin", display_name: "Nabeegh", csrf_token: "csrf-fixture" };

const rows = [
  {
    id: "trending",
    title: "Trending This Week",
    items: [
      {
        tmdb_id: 1,
        title: "Tumbbad",
        year: "2018",
        overview: "A story of greed.",
        poster_url: "https://image.tmdb.org/t/p/w500/p.jpg",
        backdrop_url: "https://image.tmdb.org/t/p/w1280/b.jpg",
        rating: 8.2,
        language: "hi",
        media_kind: "movie",
      },
    ],
  },
];

const sources = [
  {
    id: "source-1080",
    height: 1080,
    label: "1080p H265",
    size_bytes: 2_147_483_648,
    language: null,
    recommended: true,
    quality: "best",
  },
  {
    id: "source-480",
    height: 480,
    label: "480p H264",
    size_bytes: 314_572_800,
    language: null,
    recommended: false,
    quality: "lower",
  },
];

/** Route the whole authenticated browse flow against fixtures. */
function mockApi(onCreate?: (body: unknown) => void) {
  return vi.spyOn(globalThis, "fetch").mockImplementation(async (input, init) => {
    const url = String(input);
    if (url.endsWith("/auth/session")) return json(session);
    if (url.includes("/discover/search")) return json(rows[0].items);
    if (url.includes("/discover")) return json(rows);
    if (url.includes("/catalog/search")) {
      return json({
        page: 1,
        has_more: false,
        items: [
          { id: "catalog-1", title: "Tumbbad", year: "2018", media_type: "movie", season_count: null },
        ],
      });
    }
    if (url.includes("/sources")) return json(sources);
    if (url.endsWith("/jobs") && (init?.method ?? "GET") === "POST") {
      onCreate?.(JSON.parse(String(init?.body)));
      return json({ id: "job-1", title: "Tumbbad", state: "queued", version: 0 }, 202);
    }
    if (url.includes("/jobs")) return json([]);
    return new Response(null, { status: 204 });
  });
}

test("browse rows render artwork and the personal greeting", async () => {
  const fetchMock = mockApi();
  render(<App />);

  expect(await screen.findByText("Trending This Week")).toBeVisible();
  // The greeting should address the signed-in user by name.
  expect(screen.getByText(/what would you like to watch today/i)).toHaveTextContent(/nabeegh/i);
  expect(screen.getAllByRole("button", { name: /Tumbbad/ }).length).toBeGreaterThan(0);

  fetchMock.mockRestore();
});

test("choosing a title finds its sources and queues the one picked", async () => {
  const user = userEvent.setup();
  const created: unknown[] = [];
  const fetchMock = mockApi((body) => created.push(body));

  render(<App />);
  await screen.findByText("Trending This Week");
  await user.click(screen.getAllByRole("button", { name: /Tumbbad/ })[0]);

  // The modal resolves the browsed title to real, downloadable sources.
  const source = await screen.findByRole("button", { name: /1080p H265/ });
  expect(source).toBeVisible();
  // Quality is surfaced so a starved encode is visible rather than merely smaller.
  expect(screen.getByText(/low bitrate/i)).toBeVisible();

  await user.click(source);
  await waitFor(() => expect(created).toHaveLength(1));
  expect(created[0]).toMatchObject({
    catalog_id: "catalog-1",
    source_id: "source-1080",
    requested_height: 1080,
  });

  fetchMock.mockRestore();
});

test("searching replaces the rows with matching results", async () => {
  const user = userEvent.setup();
  const fetchMock = mockApi();

  render(<App />);
  await screen.findByText("Trending This Week");
  await user.type(screen.getByLabelText(/^search$/i), "Tumbbad");

  expect(await screen.findByText(/Results for/)).toBeVisible();
  fetchMock.mockRestore();
});

test("a server without discovery configured says so rather than showing nothing", async () => {
  const fetchMock = vi.spyOn(globalThis, "fetch").mockImplementation(async (input) => {
    const url = String(input);
    if (url.endsWith("/auth/session")) return json(session);
    if (url.includes("/discover")) {
      return json(
        { error: { code: "discovery_unavailable", message: "Browsing is not configured." } },
        503,
      );
    }
    return json([]);
  });

  render(<App />);
  expect(await screen.findByText(/browsing is unavailable/i)).toBeVisible();
  fetchMock.mockRestore();
});
