# Server downloads

How the server queue turns a catalog selection into a file the media library can
play. For the transfer engine itself (ranges, segmentation, resume), see
[`docs/downloads.md`](../downloads.md).

## Job lifecycle

```
queued -> resolving -> downloading -> finalizing -> ready
              |             |             |
              +-------------+-------------+--> failed / cancelled
                            |
                            +--> paused --> queued
```

One worker runs at a time (`MOVIEBOX_DOWNLOAD_CONCURRENCY`). Each job owns a
scratch directory at `_moviebox/jobs/<job-id>/` under the media root; the video
and any subtitle are staged there and only moved into `Movies/` or `Shows/` once
the transfer is verified.

## Recovering from failures

Downloads run for a long time against sources that are not always reliable, so a
failure is treated as an interruption rather than an ending.

- **Transient failures reschedule.** Provider outages, malformed responses and
  dropped transfers return the job to the queue with an exponential backoff
  (15s, 30s, 1m, 2m, 4m, capped at 5m) for up to six attempts. Bytes already on
  disk are kept and the next attempt resumes from them.
- **Permanent failures stop immediately.** A source that no longer exists, an
  identifier that fails verification, or a quality that exceeds the configured
  ceiling cannot succeed on a retry, so the job fails with that reason.
- **Expired links are re-signed.** Catalog URLs are time-limited, and a
  multi-gigabyte download can outlive several of them. Up to five expiries are
  handled by re-resolving the source and continuing, not by restarting.
- **Retry and resume are immediate.** Using retry or resume in the UI clears any
  pending backoff and restores the full attempt budget.

A job in backoff does not block the queue; the worker picks up other ready jobs
while it waits.

## Verification

Before a file is published to the library, its size on disk is compared against
the length the source advertised. A transfer that ends early is rescheduled
rather than finalized, so a truncated video is never handed to Jellyfin as a
complete item. Sources that do not advertise a length cannot be checked this way
and are accepted as-is.

## Disk space

The worker defers a job when free space would drop below
`MOVIEBOX_RESERVE_GIB`. Bytes already staged for that job count toward the
requirement, so a nearly finished download is not deferred for space it does not
actually need.

## Reclaiming partial data

Segment files for a large download are themselves large, so they are not left
behind:

- A job's scratch directory is removed once it completes, or once it fails in a
  way it cannot recover from.
- On startup, after interrupted jobs are requeued, any scratch directory that no
  longer belongs to a resumable job is deleted and the reclaimed size is logged.

Only directories named with a job id are touched; anything else under the media
root is left alone.

## Subtitles

When a download request does not name a subtitle, the server picks one
automatically in the language set by `MOVIEBOX_SUBTITLE_LANGUAGE` (English by
default; set `off` to disable). Language labels are matched across the forms
providers use — `English`, `en`, `eng`, `English (SDH)` — and a plain track is
preferred over hearing-impaired or forced variants.

The subtitle is saved beside the video as
`<video stem>.<language>.<ext>`, which is the layout Jellyfin reads as an
external subtitle stream:

```
Movies/Cocktail 2 (2026)/Cocktail 2 (2026).mkv
Movies/Cocktail 2 (2026)/Cocktail 2 (2026).English.srt
```

An explicitly requested subtitle that the source cannot provide is an error. An
automatic one is a convenience, so the video downloads without it instead. If a
job records a subtitle but no destination for it, the job completes with a
`subtitle_path_missing` warning rather than quietly omitting it.

## Library refresh

When a job reaches `ready`, the server asks Jellyfin to rescan so the new file
appears without waiting for a scheduled scan. This is best effort: if Jellyfin is
down, or no API key is configured, the download still succeeds and the file is
picked up by Jellyfin's own scan.

The API key is read from `MOVIEBOX_JELLYFIN_API_KEY_FILE` and stays inside the
Jellyfin client. It is never copied into job state, events, or logs.
