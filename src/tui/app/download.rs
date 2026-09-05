use super::App;
use crate::providers::models::ProviderKind;
use crate::tui::{action::Action, overlay::NotificationKind, state::Screen};

impl App {
    pub(super) fn start_resilient_download(
        &mut self,
        subtitle_url: Option<String>,
        source: Option<crate::providers::models::PlaybackSource>,
    ) {
        if self.state.download_progress.is_some() || self.state.active_screen != Screen::Details {
            return;
        }
        let Some(source) = source else {
            if self.state.is_fetching_streams {
                self.state.is_waiting_for_download_stream = true;
                self.state.notify(
                    NotificationKind::Info,
                    "Preparing download",
                    "Waiting for stream details.",
                );
            } else {
                self.state.notify(
                    NotificationKind::Warning,
                    "Download unavailable",
                    "Select a downloadable stream first.",
                );
            }
            return;
        };
        let headers = match crate::download::header_map(&source.headers) {
            Ok(headers) => headers,
            Err(error) => {
                self.state.notify(
                    NotificationKind::Error,
                    "Download unavailable",
                    error.to_string(),
                );
                return;
            }
        };
        let download_request = match url::Url::parse(&source.url) {
            Ok(url) => crate::download::DownloadRequest {
                url,
                headers: headers.clone(),
                maximum_redirects: 5,
                transport: crate::source::SourceTransport::HttpFile,
            },
            Err(error) => {
                self.state.notify(
                    NotificationKind::Error,
                    "Download unavailable",
                    format!("Invalid download URL: {error}"),
                );
                return;
            }
        };
        let subtitle_request = subtitle_url
            .as_deref()
            .map(|subtitle| {
                url::Url::parse(subtitle).map(|url| crate::download::DownloadRequest {
                    url,
                    headers: headers.clone(),
                    maximum_redirects: 5,
                    transport: crate::source::SourceTransport::HttpFile,
                })
            })
            .transpose();
        let subtitle_request = match subtitle_request {
            Ok(request) => request,
            Err(error) => {
                self.state.notify(
                    NotificationKind::Error,
                    "Download unavailable",
                    format!("Invalid subtitle URL: {error}"),
                );
                return;
            }
        };

        let title = self
            .state
            .selected_details
            .as_ref()
            .and_then(|details| details.get("title"))
            .and_then(|title| title.as_str())
            .unwrap_or("MovieBox-Tui_Stream");
        let media_type = self
            .state
            .selected_details
            .as_ref()
            .map(crate::tui::state::stype)
            .unwrap_or(1);
        let season = self.state.selected_season;
        let episode = self.state.selected_episode;
        let clean_title = crate::providers::moviebox::clean_moviebox_title(title);
        let safe_title = crate::download::safe_file_stem(&clean_title);

        let extension = self
            .state
            .selected_resources
            .as_ref()
            .and_then(|resources| resources.get("list"))
            .and_then(|list| list.as_array())
            .and_then(|list| list.get(self.state.resource_list_state.selected().unwrap_or(0)))
            .and_then(|resource| {
                resource
                    .get("fileName")
                    .or_else(|| resource.get("title"))
                    .and_then(|name| name.as_str())
            })
            .and_then(|name| std::path::Path::new(name).extension())
            .and_then(|extension| extension.to_str())
            .filter(|extension| {
                matches!(
                    extension.to_ascii_lowercase().as_str(),
                    "mp4" | "mkv" | "webm" | "avi" | "mov" | "m4v"
                )
            })
            .unwrap_or("mp4")
            .to_ascii_lowercase();

        let base_dir = dirs::download_dir()
            .or_else(|| dirs::home_dir().map(|h| h.join("Downloads")))
            .unwrap_or_else(|| std::path::PathBuf::from("."));

        let base_dir = if let Some(home) = dirs::home_dir() {
            let android_storage = home.join("storage/downloads");
            if android_storage.exists() {
                android_storage
            } else {
                base_dir
            }
        } else {
            base_dir
        };

        let base_dir = base_dir.join("MovieBox-TUI");
        let (target_dir, base_name) = if media_type == 2 {
            (
                base_dir
                    .join("Series")
                    .join(&safe_title)
                    .join(format!("Season {season}")),
                format!("{safe_title}_S{season:02}E{episode:02}"),
            )
        } else {
            (base_dir.join("Movies"), safe_title)
        };
        let mut destination = target_dir.join(format!("{base_name}.{extension}"));
        let mut counter = 2;
        while destination.exists() {
            destination = target_dir.join(format!("{base_name}_{counter}.{extension}"));
            counter += 1;
        }

        self.state.is_waiting_for_download_stream = false;
        self.state.download_status = Some("Preparing download...".into());
        self.state.download_progress = Some(0.0);
        self.state
            .cancel_download
            .store(false, std::sync::atomic::Ordering::SeqCst);
        self.state.notify(
            NotificationKind::Info,
            "Download started",
            "Partial data will be preserved.",
        );

        let cancel = self.state.cancel_download.clone();
        let sender = self.action_sender.clone();
        let client = crate::download::DownloadClient::builder()
            .connect_timeout(std::time::Duration::from_secs(15))
            .tcp_keepalive(std::time::Duration::from_secs(30))
            .build()
            .expect("download HTTP client configuration should be valid");

        tokio::spawn(async move {
            if let Err(error) = tokio::fs::create_dir_all(&target_dir).await {
                sender
                    .send(Action::DownloadFailed(format!(
                        "Cannot create download directory: {error}"
                    )))
                    .ok();
                return;
            }

            if let Some(subtitle_request) = subtitle_request {
                let subtitle_extension = subtitle_request
                    .url
                    .path()
                    .rsplit('.')
                    .next()
                    .map(|extension| extension.to_ascii_lowercase())
                    .filter(|extension| {
                        matches!(extension.as_str(), "srt" | "vtt" | "ass" | "ssa" | "sub")
                    })
                    .unwrap_or_else(|| "srt".to_string());
                let subtitle_path = destination.with_extension(subtitle_extension);
                let result = tokio::time::timeout(std::time::Duration::from_secs(30), async {
                    crate::download::fetch_bytes(&client, &subtitle_request).await
                })
                .await;
                match result {
                    Ok(Ok(bytes)) => {
                        if let Err(error) = tokio::fs::write(subtitle_path, bytes).await {
                            sender
                                .send(Action::SetStatus(format!(
                                    "Error: subtitle write failed: {error}"
                                )))
                                .ok();
                        }
                    }
                    Ok(Err(error)) => {
                        sender
                            .send(Action::SetStatus(format!(
                                "Error: subtitle download failed: {error}"
                            )))
                            .ok();
                    }
                    Err(_) => {
                        sender
                            .send(Action::SetStatus(
                                "Error: subtitle download timed out".to_string(),
                            ))
                            .ok();
                    }
                }
            }

            let progress_sender = sender.clone();
            let result = crate::download::download(
                &client,
                download_request,
                &destination,
                cancel,
                move |progress| {
                    let total = progress.total.unwrap_or_default();
                    let percentage = if total > 0 {
                        progress.downloaded as f64 / total as f64 * 100.0
                    } else {
                        0.0
                    };
                    let speed = progress.bytes_per_second / 1024.0 / 1024.0;
                    let eta = if total > progress.downloaded && progress.bytes_per_second > 0.0 {
                        (total - progress.downloaded) as f64 / progress.bytes_per_second
                    } else {
                        0.0
                    };
                    let status = if total > 0 {
                        format!(
                            "{:.1}/{:.1} MB | {:.1} MB/s | ETA {:.0}s | {}x | attempt {}",
                            progress.downloaded as f64 / 1024.0 / 1024.0,
                            total as f64 / 1024.0 / 1024.0,
                            speed,
                            eta,
                            progress.workers,
                            progress.attempt
                        )
                    } else {
                        format!(
                            "{:.1} MB | {:.1} MB/s | {}x | attempt {}",
                            progress.downloaded as f64 / 1024.0 / 1024.0,
                            speed,
                            progress.workers,
                            progress.attempt
                        )
                    };
                    progress_sender
                        .send(Action::UpdateDownload(Some(percentage), Some(status)))
                        .ok();
                },
            )
            .await;

            match result {
                Ok(crate::download::DownloadOutcome::Completed { .. }) => {
                    sender
                        .send(Action::DownloadCompleted(
                            destination.to_string_lossy().into_owned(),
                        ))
                        .ok();
                }
                Ok(crate::download::DownloadOutcome::Paused { .. }) => {
                    sender
                        .send(Action::DownloadPaused(
                            destination.to_string_lossy().into_owned(),
                        ))
                        .ok();
                }
                Err(error) => {
                    sender.send(Action::DownloadFailed(error.to_string())).ok();
                }
            }
        });
    }
}

impl App {
    pub(super) async fn handle_download(&mut self, action: Action) -> Option<()> {
        match action {
            Action::DownloadStream(subtitle_url) => {
                if self.state.is_resolving_playback {
                    return None;
                }
                self.state.is_resolving_playback = true;
                if self.current_subject_provider() == ProviderKind::FourKHdHub {
                    if let Some(release) = self.get_selected_release() {
                        let Some(first_mirror) = release.mirrors.first().cloned() else {
                            self.state.is_resolving_playback = false;
                            self.state.notify(
                                NotificationKind::Error,
                                "Download unavailable",
                                "No playable mirrors were found for this release.",
                            );
                            return None;
                        };
                        self.state.notify(
                            NotificationKind::Info,
                            "Preparing download",
                            "Resolving the selected mirror.",
                        );
                        let client = if release.provider == ProviderKind::BdixCircleFtp {
                            let sender_clone = self.action_sender.clone();
                            let source = crate::providers::models::PlaybackSource::bare(
                                ProviderKind::BdixCircleFtp,
                                first_mirror.resolver_url.clone(),
                                None,
                            );
                            sender_clone
                                .send(Action::StartDownload(subtitle_url, Some(source)))
                                .ok();
                            return None;
                        } else {
                            self.fourk_client.clone()
                        };
                        let sender = self.action_sender.clone();
                        tokio::spawn(async move {
                            match client.resolve_release(&release).await {
                                Ok(source) => {
                                    sender
                                        .send(Action::StartDownload(subtitle_url, Some(source)))
                                        .ok();
                                }
                                Err(error) => {
                                    log::error!("stream resolve failed: {error}");
                                    sender
                                        .send(Action::SetStatus(format!("Resolve failed: {error}")))
                                        .ok();
                                }
                            }
                        });
                    } else {
                        self.action_sender
                            .send(Action::StartDownload(subtitle_url, None))
                            .ok();
                    }
                } else {
                    let source = self.get_selected_link().map(|link| {
                        crate::providers::models::PlaybackSource::bare(
                            self.current_subject_provider(),
                            link,
                            None,
                        )
                    });
                    self.action_sender
                        .send(Action::StartDownload(subtitle_url, source))
                        .ok();
                }
                return None;
            }
            Action::StartDownload(subtitle_url, source) => {
                self.state.is_resolving_playback = false;
                self.start_resilient_download(subtitle_url, source);
                return None;
            }
            Action::PromptDownloadEpisode => {
                self.state.show_episode_download_confirm = true;
                self.state.episode_download_confirm_yes_selected = false;
            }

            Action::ConfirmDownloadEpisode => {
                self.state.show_episode_download_confirm = false;

                let subject_id = self.state.active_subject_id.clone().unwrap_or_default();
                let resource_id = self.get_selected_resource_id();

                if let Some(rid) = resource_id {
                    self.state.notify(
                        NotificationKind::Info,
                        "Preparing download",
                        "Fetching subtitles.",
                    );
                    let client = self.client.clone();
                    let sender = self.action_sender.clone();
                    tokio::spawn(async move {
                        if let Ok(res) = client.get_ext_captions(&subject_id, &rid).await {
                            sender.send(Action::ShowDownloadSubtitlePopup(res)).ok();
                        } else {
                            sender.send(Action::DownloadStream(None)).ok();
                        }
                    });
                } else {
                    self.action_sender.send(Action::DownloadStream(None)).ok();
                }
            }

            Action::PromptDownloadSeason => {
                self.state.show_season_download_confirm = true;
                self.state.season_download_confirm_yes_selected = false;
            }

            Action::ConfirmDownloadSeason => {
                self.state.show_season_download_confirm = false;
                self.state.season_subtitle_preference = None;
                let season_num = self.state.selected_season;

                let season_array_idx = self.state.available_seasons.iter().position(|s| {
                    s.get("se").and_then(|v| v.as_i64()).unwrap_or(0) as usize == season_num
                });

                if let Some(idx) = season_array_idx {
                    if idx < self.state.available_episode_numbers.len() {
                        let ep_numbers = self.state.available_episode_numbers[idx].clone();
                        self.state.download_queue.clear();

                        for ep in ep_numbers {
                            self.state.download_queue.push_back((season_num, ep));
                        }
                        self.state.download_queue_total = self.state.download_queue.len();
                        self.action_sender.send(Action::ProcessDownloadQueue).ok();
                    }
                }
            }

            Action::ProcessDownloadQueue => {
                if self.state.download_progress.is_some() {
                    let sender = self.action_sender.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                        sender.send(Action::ProcessDownloadQueue).ok();
                    });
                    return None;
                }

                if let Some((season, episode)) = self.state.download_queue.pop_front() {
                    self.state.selected_season = season;
                    self.state.selected_episode = episode;
                    let remaining = self.state.download_queue.len();
                    let total = self.state.download_queue_total;
                    let num = total - remaining;

                    self.state.notify(
                        NotificationKind::Info,
                        "Preparing episode",
                        format!("S{season:02}E{episode:02} · {num}/{total}"),
                    );

                    let subject_id = self.state.active_subject_id.clone().unwrap_or_default();

                    self.state.selected_resources = None;
                    self.state.is_fetching_streams = true;

                    self.action_sender
                        .send(Action::FetchEpisodeStreams {
                            subject_id,
                            season,
                            episode,
                            force_refresh: false,
                        })
                        .ok();

                    self.action_sender.send(Action::DownloadStream(None)).ok();
                } else if self.state.download_queue_total > 0 {
                    self.state.notify(
                        NotificationKind::Success,
                        "Season downloaded",
                        format!("{} files completed.", self.state.download_queue_total),
                    );
                    self.state.download_queue_total = 0;
                }
            }

            Action::UpdateDownload(prog, stat) => {
                if self.state.download_progress != prog || self.state.download_status != stat {
                    self.state.download_progress = prog;
                    self.state.download_status = stat;
                    self.state.dirty = true;
                }
            }
            Action::DownloadCompleted(path) => {
                self.state.download_progress = Some(100.0);
                self.state.download_status = Some("Completed".into());
                self.state.notify(
                    NotificationKind::Success,
                    "Download complete",
                    format!("Saved to {path}"),
                );
                let sender = self.action_sender.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    sender.send(Action::ClearDownload).ok();
                });
            }
            Action::DownloadFailed(error) => {
                self.state.download_progress = None;
                self.state.download_status = None;
                self.state.download_queue.clear();
                self.state.download_queue_total = 0;
                self.state.notify(
                    NotificationKind::Error,
                    "Download failed",
                    format!("Partial file preserved. {error}"),
                );
            }
            Action::DownloadPaused(path) => {
                self.state.download_progress = None;
                self.state.download_status = None;
                self.state.download_queue.clear();
                self.state.download_queue_total = 0;
                self.state.notify(
                    NotificationKind::Warning,
                    "Download paused",
                    format!("Start again to resume {path}.part"),
                );
            }
            Action::ClearDownload => {
                self.state.download_progress = None;
                self.state.download_status = None;
                if !self.state.download_queue.is_empty() {
                    self.action_sender.send(Action::ProcessDownloadQueue).ok();
                } else if self.state.download_queue_total > 0 {
                    self.state.notify(
                        NotificationKind::Success,
                        "Season downloaded",
                        format!("{} files completed.", self.state.download_queue_total),
                    );
                    self.state.download_queue_total = 0;
                }
            }
            Action::CancelDownload => {
                self.state
                    .cancel_download
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                self.state.download_status = Some("Cancelling...".to_string());
                self.state.notify(
                    NotificationKind::Warning,
                    "Cancelling download",
                    "Partial data will be preserved.",
                );
            }
            _ => return None,
        }
        None
    }
}
