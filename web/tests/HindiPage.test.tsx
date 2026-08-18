import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { HindiPage } from "../src/features/browse/HindiPage";

function json(body: unknown, status = 200) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

const title = {
  tmdb_id: 1,
  title: "Tumbbad",
  year: "2018",
  overview: null,
  poster_url: "https://image.tmdb.org/t/p/w500/p.jpg",
  backdrop_url: null,
  rating: 8.2,
  language: "hi",
  media_kind: "movie",
};

/** Capture the filter query the page actually asks for. */
function mockApi(onFilter?: (url: string) => void) {
  return vi.spyOn(globalThis, "fetch").mockImplementation(async (input) => {
    const url = String(input);
    if (url.includes("/discover/filter")) {
      onFilter?.(url);
      return json([title]);
    }
    if (url.includes("/jobs")) return json([]);
    return json([]);
  });
}

test("the section asks for Hindi films and shows what it gets", async () => {
  const seen: string[] = [];
  const fetchMock = mockApi((url) => seen.push(url));

  render(<HindiPage onSelect={() => undefined} />);
  await screen.findByRole("button", { name: /Tumbbad/ });

  expect(seen[0]).toContain("language=hi");
  expect(seen[0]).toContain("kind=movie");
  // No year chosen means no year filter, not a year of zero.
  expect(seen[0]).not.toContain("year=");
  fetchMock.mockRestore();
});

test("choosing a year narrows the request to that year", async () => {
  const user = userEvent.setup();
  const seen: string[] = [];
  const fetchMock = mockApi((url) => seen.push(url));

  render(<HindiPage onSelect={() => undefined} />);
  await screen.findByRole("button", { name: /Tumbbad/ });

  const year = String(new Date().getFullYear());
  await user.click(screen.getByRole("button", { name: year }));
  await waitFor(() => expect(seen.some((url) => url.includes(`year=${year}`))).toBe(true));
  fetchMock.mockRestore();
});

test("switching to shows changes the kind, keeping the language", async () => {
  const user = userEvent.setup();
  const seen: string[] = [];
  const fetchMock = mockApi((url) => seen.push(url));

  render(<HindiPage onSelect={() => undefined} />);
  await screen.findByRole("button", { name: /Tumbbad/ });

  await user.click(screen.getByRole("button", { name: "Shows" }));
  await waitFor(() =>
    expect(seen.some((url) => url.includes("kind=series") && url.includes("language=hi"))).toBe(
      true,
    ),
  );
  fetchMock.mockRestore();
});

test("the chosen filter is marked so the results are never ambiguous", async () => {
  const user = userEvent.setup();
  const fetchMock = mockApi();

  render(<HindiPage onSelect={() => undefined} />);
  await screen.findByRole("button", { name: /Tumbbad/ });

  expect(screen.getByRole("button", { name: "Films" })).toHaveAttribute("aria-pressed", "true");
  await user.click(screen.getByRole("button", { name: "Shows" }));
  expect(screen.getByRole("button", { name: "Shows" })).toHaveAttribute("aria-pressed", "true");
  expect(screen.getByRole("button", { name: "Films" })).toHaveAttribute("aria-pressed", "false");
  fetchMock.mockRestore();
});

test("an empty result says so rather than showing a blank grid", async () => {
  const fetchMock = vi.spyOn(globalThis, "fetch").mockImplementation(async (input) => {
    if (String(input).includes("/discover/filter")) return json([]);
    return json([]);
  });

  render(<HindiPage onSelect={() => undefined} />);
  expect(await screen.findByText(/nothing found/i)).toBeVisible();
  fetchMock.mockRestore();
});
