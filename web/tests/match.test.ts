import { findMatch, normalizeTitle } from "../src/features/browse/match";
import type { CatalogItem, DiscoverTitle } from "../src/api/types";

function browsed(title: string, year: string | null): DiscoverTitle {
  return {
    tmdb_id: 1,
    title,
    year,
    overview: null,
    poster_url: null,
    backdrop_url: null,
    rating: null,
    language: null,
  };
}

function candidate(title: string, year: string | null, id = title): CatalogItem {
  return { id, title, year, media_type: "movie", season_count: null };
}

test("a different film in the same franchise is never matched", () => {
  // The bug this guards: asking for one Spider-Man film and receiving another.
  const wanted = browsed("Spider-Man: Brand New Day", "2026");
  const results = [
    candidate("Spider-Man: Beyond Negative", "2023"),
    candidate("Spider-Man: No Way Home", "2021"),
  ];
  expect(findMatch(wanted, results)).toBeNull();
});

test("no candidates at all yields no match", () => {
  expect(findMatch(browsed("Tumbbad", "2018"), [])).toBeNull();
});

test("an exact title and year matches", () => {
  const wanted = browsed("Tumbbad", "2018");
  const match = findMatch(wanted, [candidate("Tumbbad", "2018")]);
  expect(match?.title).toBe("Tumbbad");
});

test("punctuation and case differences still match", () => {
  const wanted = browsed("Come and See", "1985");
  const match = findMatch(wanted, [candidate("Come & See", "1985")]);
  expect(match?.title).toBe("Come & See");
});

test("a leading article does not prevent a match", () => {
  const wanted = browsed("The Brutalist", "2024");
  expect(findMatch(wanted, [candidate("Brutalist", "2024")])?.year).toBe("2024");
});

test("release years one apart are accepted as the same film", () => {
  // Catalogues disagree by a year for films released across a new year.
  const wanted = browsed("Joyland", "2022");
  expect(findMatch(wanted, [candidate("Joyland", "2023")])).not.toBeNull();
});

test("a remake years apart is not treated as the same film", () => {
  const wanted = browsed("Nosferatu", "2024");
  expect(findMatch(wanted, [candidate("Nosferatu", "1922")])).toBeNull();
});

test("the exact year wins when several share a title", () => {
  const wanted = browsed("Wicked", "2024");
  const match = findMatch(wanted, [
    candidate("Wicked", "2025", "near"),
    candidate("Wicked", "2024", "exact"),
  ]);
  expect(match?.id).toBe("exact");
});

test("a series entry is not offered for a browsed movie", () => {
  const wanted = browsed("Sholay", "1975");
  const series: CatalogItem = {
    id: "s",
    title: "Sholay",
    year: "1975",
    media_type: "series",
    season_count: 2,
  };
  expect(findMatch(wanted, [series])).toBeNull();
});

test("a missing year on either side does not block a title match", () => {
  expect(findMatch(browsed("Tumbbad", null), [candidate("Tumbbad", "2018")])).not.toBeNull();
  expect(findMatch(browsed("Tumbbad", "2018"), [candidate("Tumbbad", null)])).not.toBeNull();
});

test("normalization strips accents and collapses separators", () => {
  expect(normalizeTitle("Amélie")).toBe("amelie");
  expect(normalizeTitle("Spider-Man:  Brand New Day")).toBe("spider man brand new day");
  expect(normalizeTitle("The Godfather")).toBe("godfather");
});
