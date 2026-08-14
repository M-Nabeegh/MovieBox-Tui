use super::App;
use crate::tui::{action::Action, overlay::NotificationKind, state::Screen};

impl App {
    pub(super) async fn handle_system(&mut self, action: Action) -> Option<()> {
        match action {
            Action::Tick => {
                let mut needs_redraw = (self.state.is_loading && self.state.tick_count % 5 == 0)
                    || self.state.tick_count < 15;
                self.state.tick_count = self.state.tick_count.wrapping_add(1);
                if !self.state.notifications.is_empty() {
                    needs_redraw = true;
                    self.state.expire_notifications();
                }
                if self.state.status_timer > 0 {
                    needs_redraw = true;
                    self.state.status_timer -= 1;
                    if self.state.status_timer == 0 {
                        self.state.status_message.clear();
                    }
                }
                if needs_redraw {
                    self.state.dirty = true;
                }

                let current_query = self.state.search_query.trim().to_string();
                if current_query != self.state.last_suggest_query
                    && self.state.last_search_edit.elapsed()
                        >= std::time::Duration::from_millis(350)
                {
                    self.state.last_suggest_query = current_query.clone();
                    if !current_query.is_empty() {
                        if self.state.is_tv_mode {
                            let q = current_query.to_lowercase();
                            self.state.search_suggestions = self
                                .state
                                .tv_channels
                                .iter()
                                .filter(|c| c.name.to_lowercase().contains(&q))
                                .take(10)
                                .map(|c| c.name.clone())
                                .collect();
                        } else {
                            self.action_sender.send(Action::Suggest(current_query)).ok();
                        }
                    } else {
                        self.state.search_suggestions.clear();
                    }
                }

                if self.state.pending_episode_fetch.is_some()
                    && self.state.last_episode_nav.elapsed()
                        >= std::time::Duration::from_millis(300)
                {
                    if let Some((subject_id, se, ep)) = self.state.pending_episode_fetch.take() {
                        let mut found_cached = false;
                        if let Some(pool) = self.state.stream_pool.get(&subject_id) {
                            if let Some(cached) = pool.episode_index.get(&(se, ep)) {
                                found_cached = true;
                                let count = cached.len();
                                let mut result = serde_json::Map::new();
                                result.insert(
                                    "list".to_string(),
                                    serde_json::Value::Array(cached.clone()),
                                );
                                self.state.selected_resources =
                                    Some(serde_json::Value::Object(result));
                                self.state.is_loading = false;
                                self.state.resource_list_state.select(if count > 0 {
                                    Some(0)
                                } else {
                                    None
                                });
                                self.state.set_status(
                                    format!("Resolved {} direct stream sources (cached).", count),
                                    150,
                                );
                            }
                        }

                        if !found_cached {
                            self.action_sender
                                .send(Action::FetchEpisodeStreams {
                                    subject_id,
                                    season: se,
                                    episode: ep,
                                    force_refresh: false,
                                })
                                .ok();
                        }
                    }
                }
            }

            Action::FocusChange => {
                self.prepare_image_soft_refresh();
            }

            Action::Resize(_w, _h) => {
                self.prepare_image_refresh();
                self.state.poster_protocol = None;
                self.state.search_poster_protocols.clear();
            }

            Action::SwitchProvider(provider) => self.switch_provider(provider),

            Action::ToggleHelp => {
                if matches!(self.state.active_screen, Screen::Home | Screen::Details) {
                    self.state.show_help = !self.state.show_help;
                    if self.state.show_help {
                        self.state.show_theme_popup = false;
                        self.state.show_browse_popup = false;
                        self.state.tv_config_popup = false;
                        self.state.player_picker_popup = false;
                        self.state.subtitle_popup = false;
                        self.state.is_download_subtitle_popup = false;
                        self.state.show_season_download_confirm = false;
                        self.state.show_episode_download_confirm = false;
                    }
                }
            }

            Action::Refresh => match self.state.active_screen {
                Screen::Home => {
                    let query = self.state.search_query.trim().to_string();
                    if self.state.is_tv_mode {
                        self.state
                            .set_status("Reloading TV playlists...".to_string(), 150);
                        self.reload_tv_playlists();
                    } else if let Some(preset) = self.state.active_browse_preset {
                        self.state.is_loading = true;
                        self.state
                            .set_status(format!("Reloading {}...", preset.label()), 150);
                        self.action_sender
                            .send(Action::FetchHomepage {
                                tab_id: "2".to_string(),
                                page: 1,
                            })
                            .ok();
                    } else if !query.is_empty() {
                        self.action_sender
                            .send(Action::Search {
                                query,
                                force_refresh: true,
                            })
                            .ok();
                    }
                }
                Screen::Details => {
                    if let Some(id) = self.state.active_subject_id.clone() {
                        let se = if self.state.available_seasons.is_empty() {
                            0
                        } else {
                            self.state.selected_season
                        };
                        let ep = if self.state.available_seasons.is_empty() {
                            0
                        } else {
                            self.state.selected_episode
                        };
                        let id_clone = id.clone();
                        let id_clone_2 = id.clone();
                        let provider = self.provider_for_subject(&id);
                        tokio::task::spawn_blocking(move || {
                            crate::cache::invalidate_provider_stream_cache(
                                provider, &id_clone, se, ep,
                            );
                            crate::cache::invalidate_provider_details_cache(provider, &id_clone_2);
                        });
                        self.state.selected_season = se;
                        self.state.selected_episode = ep;

                        self.action_sender
                            .send(Action::FetchDetails(id.clone(), true))
                            .ok();

                        self.action_sender
                            .send(Action::FetchEpisodeStreams {
                                subject_id: id,
                                season: se,
                                episode: ep,
                                force_refresh: true,
                            })
                            .ok();
                    }
                }
                _ => {}
            },

            Action::ClearCache => {
                let sender = self.action_sender.clone();
                tokio::spawn(async move {
                    let result = tokio::task::spawn_blocking(|| {
                        crate::cache::clear_all_cache();
                        Ok::<(), String>(())
                    })
                    .await
                    .map_err(|error| format!("cache clear task failed: {error}"))
                    .and_then(|result| result);
                    sender.send(Action::CacheCleared(result)).ok();
                });
                self.state.stream_pool.clear();
                self.state.image_cache.clear();
                self.state.search_posters.clear();
                self.state.search_poster_protocols.clear();
                self.state.browse_metrics.clear();
                self.state.preview_cache.clear();
                self.state.poster_image = None;
                self.state.poster_protocol = None;
                self.state.search_preview = None;
                self.state.search_error = None;
                self.state.search_list_state.select(None);
                self.state.selected_resources = None;
                self.state.active_subject_id = None;
                self.state.selected_details = None;
                self.state.details_pane = crate::tui::state::DetailsPane::default();
                self.state.selected_season = 1;
                self.state.selected_episode = 1;
                self.state.language_chosen = false;
                self.state.season_list_state.select(None);
                self.state.episode_list_state.select(None);
                self.state.language_list_state.select(None);
                self.state.available_seasons.clear();
                self.state.available_episode_numbers.clear();
                if self.state.is_tv_mode {
                    self.state.tv_channels.clear();
                    self.state.search_results.clear();
                }
                self.prepare_image_refresh();
                self.state.set_status("Clearing cache...".to_string(), 150);
            }

            Action::CacheCleared(result) => match result {
                Ok(()) => {
                    if self.state.is_tv_mode && !self.state.tv_playlists.is_empty() {
                        self.state.set_status(
                            "Cache cleared. Reloading TV playlists...".to_string(),
                            150,
                        );
                        self.reload_tv_playlists();
                    } else {
                        self.state
                            .set_status("Cache cleared completely.".to_string(), 150);
                    }
                }
                Err(error) => {
                    log::error!("cache clear failed: {error}");
                    self.state
                        .notify(NotificationKind::Error, "Cache clear failed", error);
                }
            },

            Action::ToggleThemePopup => {
                let open = !self.state.show_theme_popup;
                if open {
                    self.reset_transient_overlays();
                    self.state.tv_config_popup = false;
                    self.state.show_theme_popup = true;
                    if let Some(idx) = crate::tui::theme::AVAILABLE_THEMES
                        .iter()
                        .position(|&t| t.eq_ignore_ascii_case(&self.state.active_theme_kind))
                    {
                        self.state.theme_list_state.select(Some(idx));
                    } else {
                        self.state.theme_list_state.select(Some(0));
                    }
                } else {
                    self.state.show_theme_popup = false;
                }
            }

            Action::ShowBrowseMenu => {
                if self.state.is_tv_mode
                    || self.state.active_provider
                        != crate::providers::models::ProviderKind::MovieBox
                {
                    self.state.set_status(
                        "Browse is available only with the MovieBox provider.".to_string(),
                        180,
                    );
                } else {
                    self.reset_transient_overlays();
                    self.state.show_browse_popup = true;
                    self.state.browse_list_state.select(Some(0));
                    self.state.input_mode = crate::tui::state::InputMode::Normal;
                }
            }

            Action::SelectTheme(theme_name) => {
                let kind = crate::tui::theme::ThemeKind::parse(&theme_name);
                self.state.active_theme_kind = kind.as_str().to_string();
                self.theme = crate::tui::theme::Theme::from_kind(kind);
                self.persist_config();
                self.state.dirty = true;
            }

            Action::SetStatus(msg) => {
                self.state.is_resolving_playback = false;
                if msg.starts_with("Error:") {
                    log::error!("{msg}");
                    self.state.notify(
                        NotificationKind::Error,
                        "Operation failed",
                        msg.trim_start_matches("Error:").trim(),
                    );
                } else {
                    self.state.set_status(msg, 150);
                }
            }

            Action::CheckForUpdates => {
                let update_sender = self.action_sender.clone();
                tokio::spawn(async move {
                    let start = tokio::time::Instant::now();
                    let result = crate::tui::updater::check(env!("CARGO_PKG_VERSION")).await;

                    let elapsed = start.elapsed();
                    if elapsed.as_millis() < 1500 {
                        tokio::time::sleep(std::time::Duration::from_millis(1500) - elapsed).await;
                    }

                    match result {
                        Ok(Some((version, notes))) => {
                            update_sender
                                .send(Action::UpdateAvailable(version, notes))
                                .ok();
                        }
                        Ok(None) => {
                            update_sender
                                .send(Action::UpdateAvailable("none".into(), "".into()))
                                .ok();
                        }
                        Err(error) => {
                            update_sender
                                .send(Action::UpdateAvailable(
                                    format!("error:{}", error),
                                    "".into(),
                                ))
                                .ok();
                        }
                    }
                });
            }

            Action::UpdateAvailable(version, notes) => {
                if self.state.active_screen == Screen::Startup {
                    self.state.active_screen = Screen::Home;
                }

                if version == "none" {
                    self.state.last_update_check = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();
                    self.persist_config();
                    if self.state.manual_update_check {
                        self.state.notify(
                            NotificationKind::Success,
                            "Up to date",
                            "You are using the latest version.",
                        );
                    }
                    self.state.manual_update_check = false;
                } else if version.starts_with("error:") {
                    let err = version.trim_start_matches("error:");
                    if self.state.manual_update_check {
                        self.state.notify(
                            NotificationKind::Error,
                            "Update check failed",
                            err.to_string(),
                        );
                    }
                    self.state.manual_update_check = false;
                } else {
                    self.state.last_update_check = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();
                    self.persist_config();
                    self.state.manual_update_check = false;
                    self.state.update_available = Some((version, notes));
                }
            }
            _ => return None,
        }
        None
    }
}
