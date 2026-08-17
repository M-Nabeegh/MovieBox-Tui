import type { CatalogItem, DiscoverTitle } from "../../api/types";

/**
 * Matching a browsed title to a downloadable one.
 *
 * Browsing and downloading use different catalogues, so a chosen title has to
 * be found again in the provider's index. Getting this wrong is expensive and
 * silent: the user asks for one film and gigabytes of a different one arrive.
 * So a match must be justified, and anything less returns nothing rather than
 * the nearest thing to hand.
 */

import type { Job } from "../../api/types";

/** Release dates differ between catalogues by up to a year for the same film. */
const YEAR_TOLERANCE = 1;

/** How a browsed title relates to what is already on the server. */
export type LibraryState = "ready" | "downloading" | null;

/**
 * Whether a browsed title is already downloaded or on its way.
 *
 * Queueing something twice wastes bandwidth and only fails once the worker gets
 * to it, so browse marks what is already accounted for.
 */
export function libraryStateFor(title: DiscoverTitle, jobs: Job[]): LibraryState {
  const wanted = normalizeTitle(title.title);
  const matching = jobs.filter((job) => normalizeTitle(job.title) === wanted);
  if (matching.some((job) => job.state === "ready")) return "ready";
  if (
    matching.some((job) =>
      ["queued", "resolving", "downloading", "paused", "finalizing"].includes(job.state),
    )
  ) {
    return "downloading";
  }
  return null;
}

/**
 * Reduce a title to its comparable core.
 *
 * Catalogues disagree about punctuation, accents, and leading articles, none of
 * which distinguish one film from another.
 */
export function normalizeTitle(value: string): string {
  return value
    .normalize("NFKD")
    .replace(/[̀-ͯ]/g, "")
    .toLowerCase()
    .replace(/&/g, " and ")
    .replace(/[^a-z0-9]+/g, " ")
    .trim()
    .replace(/^(the|a|an) /, "");
}

function yearsAgree(wanted: string | null, candidate: string | null): boolean {
  if (!wanted || !candidate) return true;
  const a = Number.parseInt(wanted, 10);
  const b = Number.parseInt(candidate, 10);
  if (Number.isNaN(a) || Number.isNaN(b)) return true;
  return Math.abs(a - b) <= YEAR_TOLERANCE;
}

/**
 * Find the catalogue entry that is genuinely the same film, or `null`.
 *
 * The title must match exactly once normalized. A near-miss is still a
 * different film — "Brand New Day" and "Beyond Negative" share a franchise, not
 * an identity — so partial overlap never qualifies.
 */
export function findMatch(
  title: DiscoverTitle,
  candidates: CatalogItem[],
): CatalogItem | null {
  const wanted = normalizeTitle(title.title);
  if (!wanted) return null;

  // A show and a film can share a name — a series must not satisfy a request
  // for the film, or vice versa.
  const wantedType = title.media_kind === "series" ? "series" : "movie";
  const sameTitle = candidates.filter(
    (item) => item.media_type === wantedType && normalizeTitle(item.title) === wanted,
  );
  if (sameTitle.length === 0) return null;

  // With the title settled, the year decides between re-releases and remakes.
  // A long-running show's listed year drifts between catalogues, so the year is
  // only decisive for films.
  const sameYear =
    wantedType === "series"
      ? sameTitle
      : sameTitle.filter((item) => yearsAgree(title.year, item.year));
  if (sameYear.length === 0) return null;

  // Prefer an exact year over one merely within tolerance.
  return (
    sameYear.find((item) => item.year && title.year && item.year === title.year) ??
    sameYear[0]
  );
}
