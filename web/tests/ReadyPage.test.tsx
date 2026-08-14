import { render, screen } from "@testing-library/react";
import { ReadyPage } from "../src/features/library/ReadyPage";

test("explains ready media and keeps Jellyfin as a Task 12 placeholder", () => {
  render(<ReadyPage job={{ id: "ready-1", catalog_id: "catalog-1", source_id: "source-1", subtitle_id: null, title: "Ready Movie", year: "2024", media_type: "movie", season: null, episode: null, episode_title: null, state: "ready", requested_height: 1080, downloaded_bytes: 100, total_bytes: 100, speed_bytes_per_second: null, attempt: 1, error_code: null, error_message: null, warning: null, version: 1 }} />);

  expect(screen.getByText("Ready Movie")).toBeVisible();
  expect(screen.getByText(/Jellyfin will play this from your media library/i)).toBeVisible();
  expect(screen.getByRole("link", { name: "Open in Jellyfin" })).toHaveAttribute("href", "#");
});
