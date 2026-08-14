use super::{App, network};
use crate::providers::models::{ProviderKind, RequestContext};
use crate::tui::{
    action::Action,
    overlay::NotificationKind,
    state::{BrowseMetrics, BrowsePreset, InputMode, Screen, SearchResult},
};

fn metric_value(item: &serde_json::Value, keys: &[&str]) -> Option<f64> {
    let mut containers = vec![item];
    if let Some(metadata) = item.get("metadata") {
        containers.push(metadata);
    }
    if let Some(meta) = item.get("meta") {
        containers.push(meta);
    }

    containers.into_iter().find_map(|container| {
        keys.iter().find_map(|key| {
            let value = container.get(*key)?;
            value
                .as_f64()
                .or_else(|| value.as_i64().map(|number| number as f64))
                .or_else(|| value.as_str().and_then(|text| text.parse::<f64>().ok()))
        })
    })
}

fn extract_browse_metrics(item: &serde_json::Value) -> BrowseMetrics {
    BrowseMetrics {
        trending: metric_value(
            item,
            &["__browse_rank", "imdb_trending", "imdbTrending", "trending"],
        ),
        rating: metric_value(
            item,
            &["imdbRatingValue", "imdbRate", "imdb_rating", "imdbRating"],
        ),
        recent_rating: metric_value(
            item,
            &[
                "imdb_rating_30d",
                "imdbRating30Days",
                "imdbRatingLast30Days",
                "imdb_rating_recent",
                "imdbRatingValue",
                "imdbRate",
            ],
        ),
        popularity: metric_value(
            item,
            &[
                "__browse_rank",
                "imdb_popularity",
                "imdbPopularity",
                "popularity",
                "viewers",
            ],
        ),
    }
}

fn browse_group_matches(title: &str, preset: BrowsePreset) -> bool {
    let title = title.to_lowercase();
    match preset.metric() {
        crate::tui::state::BrowseMetric::Trending => {
            title.contains("trending") || title.contains("hot")
        }
        crate::tui::state::BrowseMetric::Rating => title.contains("top") || title.contains("rated"),
        crate::tui::state::BrowseMetric::RecentRating => {
            title.contains("new") || title.contains("release") || title.contains("recent")
        }
        crate::tui::state::BrowseMetric::Popularity => {
            title.contains("popular") || title.contains("most") || title.contains("watched")
        }
    }
}

fn compare_browse_values(
    left: Option<f64>,
    right: Option<f64>,
    descending: bool,
) -> std::cmp::Ordering {
    match (left, right) {
        (Some(left), Some(right)) => {
            let order = left
                .partial_cmp(&right)
                .unwrap_or(std::cmp::Ordering::Equal);
            if descending { order.reverse() } else { order }
        }
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    }
}

impl App {
    pub(super) fn apply_tv_search_results(&mut self, query: &str, lower_query: &str) {
        self.state.search_results = self
            .state
            .tv_channels
            .iter()
            .filter(|channel| {
                lower_query == "/list"
                    || channel.name.to_lowercase().contains(lower_query)
                    || channel.group.to_lowercase().contains(lower_query)
            })
            .map(|channel| SearchResult {
                id: channel.stream_url.clone(),
                title: channel.name.clone(),
                stype: 3,
                release_year: channel.group.clone(),
                cover_url: Some(channel.logo.clone()),
                season: 1,
                episode: 1,
                provider: ProviderKind::MovieBox,
            })
            .collect();
        self.state.is_loading = false;
        self.state
            .search_list_state
            .select(if self.state.search_results.is_empty() {
                None
            } else {
                Some(0)
            });
        if !self.state.search_results.is_empty() {
            self.spawn_search_posters(
                self.state
                    .search_results
                    .iter()
                    .take(15)
                    .map(|result| (result.id.clone(), result.cover_url.clone()))
                    .collect(),
            );
        }
        self.state.set_status(
            if self.state.search_results.is_empty() {
                format!("No matches for '{}'.", query)
            } else {
                format!("Found {} channels.", self.state.search_results.len())
            },
            150,
        );
    }

    pub(super) fn handle_search_command(&mut self, query: &str, lower_query: &str) -> Option<bool> {
        if lower_query == "/clear-cache" {
            self.action_sender.send(Action::ClearCache).ok();
            self.state.search_query.clear();
            return Some(true);
        }
        if lower_query == "/github" {
            let _ = open::that("https://github.com/mesamirh/MovieBox-Tui");
            self.state.search_query.clear();
            self.state.input_mode = InputMode::Normal;
            return Some(true);
        }
        if lower_query == "/update" {
            self.state.search_query.clear();
            self.state.input_mode = InputMode::Normal;
            self.state.active_screen = Screen::Startup;
            self.state.update_available = None;
            self.state.manual_update_check = true;
            self.action_sender.send(Action::CheckForUpdates).ok();
            return Some(true);
        }
        if lower_query == "/theme" || lower_query == "/themes" {
            self.state.search_query.clear();
            self.state.input_mode = InputMode::Normal;
            self.action_sender.send(Action::ToggleThemePopup).ok();
            return Some(true);
        }
        if lower_query == "/browse" {
            self.state.search_query.clear();
            self.state.input_mode = InputMode::Normal;
            self.action_sender.send(Action::ShowBrowseMenu).ok();
            return Some(true);
        }
        if lower_query == "/toggle-update" {
            self.state.auto_update = !self.state.auto_update;
            self.persist_config();
            self.state.search_query.clear();
            self.state.input_mode = InputMode::Normal;
            self.state.notify(
                NotificationKind::Info,
                "Automatic updates",
                if self.state.auto_update {
                    "Enabled"
                } else {
                    "Disabled"
                },
            );
            return Some(true);
        }
        if lower_query == "/enable-bdix" || lower_query == "/disable-bdix" {
            let enable_req = lower_query == "/enable-bdix";
            if self.state.bdix_enabled == enable_req {
                self.state.search_query.clear();
                self.state.input_mode = InputMode::Normal;
                self.state.notify(
                    NotificationKind::Info,
                    "BDIX Providers",
                    if enable_req {
                        "Already Enabled"
                    } else {
                        "Already Disabled"
                    },
                );
                return Some(true);
            }

            self.state.bdix_enabled = enable_req;
            self.persist_config();
            self.state.search_query.clear();
            self.state.input_mode = InputMode::Normal;
            self.state.notify(
                NotificationKind::Info,
                "BDIX Providers",
                if self.state.bdix_enabled {
                    "Enabled"
                } else {
                    "Disabled"
                },
            );
            if !self.state.bdix_enabled && self.state.active_provider.is_bdix() {
                let new_provider = ProviderKind::ENABLED
                    .iter()
                    .copied()
                    .find(|provider| !provider.is_bdix())
                    .unwrap_or(ProviderKind::MovieBox);
                self.action_sender
                    .send(Action::SwitchProvider(new_provider))
                    .ok();
            }
            return Some(true);
        }

        if self.state.is_tv_mode {
            if lower_query == "/config" {
                self.action_sender.send(Action::ShowTvConfig).ok();
                self.state.search_query.clear();
                return Some(true);
            }
            if lower_query.starts_with('/') && lower_query != "/list" {
                self.state.set_status(
                    "Switch to streaming mode to use this command".to_string(),
                    150,
                );
                self.state.search_query.clear();
                return Some(true);
            }

            self.apply_tv_search_results(query, lower_query);
            return Some(true);
        }

        if let Some(tid) = match lower_query {
            "/home" | "/discover" => Some("0"),
            "/movies" => Some("2"),
            "/shows" | "/tvshows" => Some("5"),
            "/anime" => Some("8"),
            _ => None,
        } {
            if self.state.active_provider != ProviderKind::MovieBox {
                self.state.set_status(
                    "This provider has no shared discover feed; enter a title to search.",
                    180,
                );
                return Some(true);
            }
            self.state.active_browse_preset = None;
            self.action_sender
                .send(Action::FetchHomepage {
                    tab_id: tid.to_string(),
                    page: 1,
                })
                .ok();
            return Some(true);
        }

        None
    }

    pub(super) fn prepare_search_request(&mut self, query: &str) -> RequestContext {
        self.state.active_search_request = self.state.active_search_request.wrapping_add(1);
        self.state.active_preview_request = self.state.active_preview_request.wrapping_add(1);
        self.state.is_homepage_mode = false;
        self.state.active_browse_preset = None;
        self.state.browse_metrics.clear();
        self.state.current_page = 1;
        self.state.active_screen = Screen::Home;
        self.state.active_subject_id = None;
        self.state.selected_details = None;
        self.state.selected_resources = None;
        self.state.is_loading = true;
        self.state.search_error = None;
        self.state.search_list_state.select(Some(0));
        self.state.search_suggestions.clear();
        self.state.suggest_index = None;
        self.state.search_preview = None;
        self.state.preview_loading = false;
        self.state.poster_image = None;
        self.state.poster_protocol = None;
        self.state
            .set_status(format!("Searching for '{}'...", query), 150);
        self.request_context()
    }

    pub(super) fn run_search_request(
        &self,
        query: String,
        force_refresh: bool,
        context: RequestContext,
    ) {
        let request_id = self.state.active_search_request;
        let page = 1;
        let sender = self.action_sender.clone();
        let client = self.client.clone();
        let fourk_client = self.fourk_client.clone();
        let circleftp_client = self.circleftp_client.clone();
        let dhakaflix_client = self.dhakaflix_client.clone();
        tokio::spawn(async move {
            if !force_refresh {
                let q = query.clone();
                let provider = context.provider;
                if let Ok(Some(cached)) = tokio::task::spawn_blocking(move || {
                    crate::cache::get_provider_search_cache(provider, &q)
                })
                .await
                {
                    sender
                        .send(Action::SearchSuccess {
                            context,
                            request_id,
                            query: query.clone(),
                            page,
                            payload: cached,
                        })
                        .ok();
                    return;
                }
            }

            let result = network::provider_search(
                &client,
                &fourk_client,
                &circleftp_client,
                &dhakaflix_client,
                context.provider,
                &query,
                page,
            )
            .await;
            match result {
                Ok(res) => {
                    let q = query.clone();
                    let provider = context.provider;
                    let cached = res.clone();
                    tokio::task::spawn_blocking(move || {
                        crate::cache::set_provider_search_cache(provider, &q, &cached);
                    });
                    sender
                        .send(Action::SearchSuccess {
                            context,
                            request_id,
                            query,
                            page,
                            payload: res,
                        })
                        .ok();
                }
                Err(error) => {
                    sender
                        .send(Action::SearchFailure(context, request_id, page, error))
                        .ok();
                }
            }
        });
    }

    pub(super) fn prepare_homepage_request(&mut self, tab_id: &str, page: usize) {
        self.state.active_homepage_request = self.state.active_homepage_request.wrapping_add(1);
        self.state.active_preview_request = self.state.active_preview_request.wrapping_add(1);
        self.state.is_homepage_mode = true;
        self.state.current_tab_id = tab_id.to_string();
        self.state.current_page = page;
        self.state.active_screen = Screen::Home;
        self.state.active_subject_id = None;
        self.state.selected_details = None;
        self.state.selected_resources = None;
        self.state.is_loading = true;
        self.state.search_error = None;
        if page == 1 {
            self.state.search_results.clear();
            self.state.browse_metrics.clear();
            self.state.search_list_state.select(Some(0));
        }
        self.state.search_suggestions.clear();
        self.state.suggest_index = None;
        self.state.search_preview = None;
        self.state.preview_loading = false;
        self.state.poster_image = None;
        self.state.poster_protocol = None;
        self.state
            .set_status("Loading discover tab...".to_string(), 150);
    }

    pub(super) fn prepare_details_request(&mut self, id: &str) -> RequestContext {
        self.state.poster_protocol = None;
        self.state.is_loading = true;
        self.state.active_details_request = self.state.active_details_request.wrapping_add(1);
        self.state
            .fetch_cancel
            .store(false, std::sync::atomic::Ordering::Relaxed);
        self.state
            .set_status("Fetching details...".to_string(), 150);
        self.state.stream_pool.clear();

        let provider = self.provider_for_subject(id);
        let mut context = self.request_context();
        context.provider = provider;
        context
    }

    pub(super) fn run_details_request(
        &self,
        id: String,
        force_refresh: bool,
        context: RequestContext,
    ) {
        let request_id = self.state.active_details_request;
        let client = self.client.clone();
        let fourk_client = self.fourk_client.clone();
        let circleftp_client = self.circleftp_client.clone();
        let dhakaflix_client = self.dhakaflix_client.clone();
        let sender = self.action_sender.clone();
        tokio::spawn(async move {
            if !force_refresh {
                let id_for_cache = id.clone();
                if let Ok(Some(cached)) = tokio::task::spawn_blocking(move || {
                    crate::cache::get_provider_details_cache(context.provider, &id_for_cache)
                })
                .await
                {
                    sender
                        .send(Action::DetailsSuccess(
                            context,
                            request_id,
                            id.clone(),
                            cached,
                        ))
                        .ok();
                    return;
                }
            }

            let result = network::provider_details(
                &client,
                &fourk_client,
                &circleftp_client,
                &dhakaflix_client,
                context.provider,
                &id,
            )
            .await;
            match result {
                Ok(details) => {
                    let id_for_cache = id.clone();
                    let details_for_cache = details.clone();
                    let _ = tokio::task::spawn_blocking(move || {
                        crate::cache::set_provider_details_cache(
                            context.provider,
                            &id_for_cache,
                            &details_for_cache,
                        )
                    })
                    .await;
                    sender
                        .send(Action::DetailsSuccess(context, request_id, id, details))
                        .ok();
                }
                Err(error) => {
                    sender
                        .send(Action::DetailsFailure(context, request_id, error))
                        .ok();
                }
            }
        });
    }

    pub(super) fn run_homepage_request(&self, tab_id: String, page: usize) {
        let request_id = self.state.active_homepage_request;
        let client = self.client.clone();
        let sender = self.action_sender.clone();
        tokio::spawn(async move {
            let t_clone = tab_id.clone();
            if let Ok(Some(cached)) = tokio::task::spawn_blocking(move || {
                crate::cache::get_homepage_cache(&t_clone, page)
            })
            .await
            {
                sender
                    .send(Action::HomepageSuccess {
                        request_id,
                        tab_id: tab_id.clone(),
                        page,
                        payload: cached,
                    })
                    .ok();
                return;
            }

            match client.get_homepage(&tab_id, page).await {
                Ok(res) => {
                    let r_clone = res.clone();
                    let t_clone = tab_id.clone();
                    tokio::task::spawn_blocking(move || {
                        crate::cache::set_homepage_cache(&t_clone, page, &r_clone);
                    });
                    sender
                        .send(Action::HomepageSuccess {
                            request_id,
                            tab_id,
                            page,
                            payload: res,
                        })
                        .ok();
                }
                Err(error) => {
                    sender
                        .send(Action::HomepageFailure(request_id, format!("{:?}", error)))
                        .ok();
                }
            }
        });
    }

    pub(super) fn extract_homepage_subjects(payload: &serde_json::Value) -> Vec<serde_json::Value> {
        let mut extracted_subjects = Vec::new();
        if let Some(items) = payload.get("items").and_then(|i| i.as_array()) {
            for item in items {
                if let Some(banner) = item
                    .get("banner")
                    .and_then(|b| b.get("banners"))
                    .and_then(|b| b.as_array())
                {
                    for banner_item in banner {
                        if let Some(subject) = banner_item.get("subject") {
                            extracted_subjects.push(subject.clone());
                        }
                    }
                }
                if let Some(custom_data) = item
                    .get("customData")
                    .and_then(|c| c.get("items"))
                    .and_then(|i| i.as_array())
                {
                    for custom_item in custom_data {
                        if let Some(subject) = custom_item.get("subject") {
                            extracted_subjects.push(subject.clone());
                        }
                    }
                }
                if let Some(subjects) = item.get("subjects").and_then(|s| s.as_array()) {
                    for subject in subjects {
                        extracted_subjects.push(subject.clone());
                    }
                }
            }
        }
        extracted_subjects
    }

    pub(super) fn extract_browse_subjects(
        payload: &serde_json::Value,
        preset: BrowsePreset,
    ) -> Vec<serde_json::Value> {
        let Some(items) = payload.get("items").and_then(|items| items.as_array()) else {
            return Vec::new();
        };

        let matching_items: Vec<_> = items
            .iter()
            .filter(|item| {
                item.get("title")
                    .and_then(|title| title.as_str())
                    .is_some_and(|title| browse_group_matches(title, preset))
            })
            .collect();
        let groups = if matching_items.is_empty() {
            items.iter().collect()
        } else {
            matching_items
        };

        let mut subjects = Vec::new();
        for group in groups {
            let Some(group_subjects) = group.get("subjects").and_then(|s| s.as_array()) else {
                continue;
            };
            let rank_metric = preset.metric() == crate::tui::state::BrowseMetric::Trending;
            for (index, subject) in group_subjects.iter().enumerate() {
                let mut subject = subject.clone();
                if rank_metric && let Some(subject_object) = subject.as_object_mut() {
                    subject_object.insert(
                        "__browse_rank".to_string(),
                        serde_json::json!((group_subjects.len() - index) as f64),
                    );
                }
                subjects.push(subject);
            }
        }
        subjects
    }

    pub(super) fn append_homepage_subjects(&mut self, subjects: Vec<serde_json::Value>) -> usize {
        let mut count = 0;
        for item in subjects {
            let id = item
                .get("subjectId")
                .and_then(|si| si.as_str())
                .unwrap_or("")
                .to_string();
            let raw_title = item
                .get("title")
                .and_then(|t| t.as_str())
                .unwrap_or("Unknown")
                .to_string();
            let clean_title = crate::providers::moviebox::clean_moviebox_title(&raw_title);
            let stype = item
                .get("subjectType")
                .and_then(|st| st.as_i64())
                .unwrap_or(0);
            let release_year = item
                .get("releaseDate")
                .and_then(|rd| rd.as_str())
                .unwrap_or("")
                .split('-')
                .next()
                .unwrap_or("")
                .to_string();
            let cover_url = item
                .get("cover")
                .and_then(|c| c.get("url"))
                .and_then(|u| u.as_str())
                .map(|s| s.to_string());
            let season = item.get("season").and_then(|s| s.as_u64()).unwrap_or(0) as usize;
            let metrics = extract_browse_metrics(&item);

            if let Some(existing) = self.state.search_results.iter_mut().find(|r| r.id == id) {
                let stored_metrics = self.state.browse_metrics.entry(id.clone()).or_default();
                stored_metrics.trending = stored_metrics.trending.or(metrics.trending);
                stored_metrics.rating = stored_metrics.rating.or(metrics.rating);
                stored_metrics.recent_rating =
                    stored_metrics.recent_rating.or(metrics.recent_rating);
                stored_metrics.popularity = stored_metrics.popularity.or(metrics.popularity);
                if season > existing.season {
                    existing.season = season;
                    existing.title = clean_title;
                    existing.stype = stype;
                    existing.release_year = release_year;
                    existing.cover_url = cover_url;
                }
                continue;
            }

            let raw_lower = raw_title.to_lowercase();
            let is_dub = raw_lower.contains("[hindi]")
                || raw_lower.contains("[tamil]")
                || raw_lower.contains("[telugu]")
                || raw_lower.contains("[english]");

            if is_dub
                && self
                    .state
                    .search_results
                    .iter()
                    .any(|r| r.title == clean_title && r.stype == stype)
            {
                continue;
            }

            if self.state.search_results.iter().any(|r| {
                r.title == clean_title && r.release_year == release_year && r.stype == stype
            }) {
                continue;
            }

            if !id.is_empty() {
                self.state.browse_metrics.insert(id.clone(), metrics);
                self.state.search_results.push(SearchResult {
                    id,
                    title: clean_title,
                    stype,
                    release_year,
                    cover_url,
                    season,
                    episode: 1,
                    provider: ProviderKind::MovieBox,
                });
                count += 1;
            }
        }
        count
    }

    pub(super) fn sort_browse_results(&mut self) {
        let Some(preset) = self.state.active_browse_preset else {
            return;
        };
        let metrics = self.state.browse_metrics.clone();
        let metric = preset.metric();
        let descending = preset.descending();
        self.state.search_results.sort_by(|left, right| {
            let left_value = metrics
                .get(&left.id)
                .and_then(|values| values.value(metric));
            let right_value = metrics
                .get(&right.id)
                .and_then(|values| values.value(metric));
            let metric_order = compare_browse_values(left_value, right_value, descending);
            if left_value.is_none() && right_value.is_none() {
                metric_order
            } else {
                metric_order
                    .then_with(|| left.title.to_lowercase().cmp(&right.title.to_lowercase()))
            }
        });
    }

    pub(super) fn spawn_search_posters(&self, results: Vec<(String, Option<String>)>) {
        let sender = self.action_sender.clone();
        let req_client = self.client.http_client().clone();
        tokio::spawn(async move {
            let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(4));
            for (id, cover_url) in results {
                let Some(url) = cover_url else {
                    continue;
                };
                if url.is_empty() {
                    continue;
                }
                let permit = sem.clone().acquire_owned().await.ok();
                let tx = sender.clone();
                let client = req_client.clone();
                tokio::spawn(async move {
                    let _permit = permit;
                    if let Some(bytes) = network::fetch_poster_bytes(&client, &url).await
                        && let Some(img) = network::decode_poster(bytes).await
                    {
                        tx.send(Action::SearchPosterLoaded(id, Some(img))).ok();
                    }
                });
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_browse_metrics_from_string_and_nested_values() {
        let item = serde_json::json!({
            "metadata": {
                "imdb_trending": "9.2",
                "imdbRatingValue": "7.2",
                "imdbRatingLast30Days": 8.7
            },
            "imdb_popularity": 74
        });

        let metrics = extract_browse_metrics(&item);
        assert_eq!(metrics.trending, Some(9.2));
        assert_eq!(metrics.recent_rating, Some(8.7));
        assert_eq!(metrics.popularity, Some(74.0));
        assert_eq!(metrics.rating, Some(7.2));
    }

    #[test]
    fn browse_sort_keeps_unranked_titles_at_the_end() {
        assert_eq!(
            compare_browse_values(Some(7.0), Some(8.0), true),
            std::cmp::Ordering::Greater
        );
        assert_eq!(
            compare_browse_values(Some(7.0), Some(8.0), false),
            std::cmp::Ordering::Less
        );
        assert_eq!(
            compare_browse_values(Some(7.0), None, true),
            std::cmp::Ordering::Less
        );
    }

    #[test]
    fn browse_extraction_uses_matching_curated_group() {
        let payload = serde_json::json!({
            "items": [
                {
                    "title": "Trending in Cinema",
                    "subjects": [
                        {"subjectId": "trending-1", "title": "Current hit"}
                    ]
                },
                {
                    "title": "Top 20 Movies",
                    "subjects": [
                        {"subjectId": "top-1", "title": "All-time favorite"}
                    ]
                }
            ]
        });

        let subjects = App::extract_browse_subjects(&payload, BrowsePreset::TrendingDesc);
        assert_eq!(subjects.len(), 1);
        assert_eq!(subjects[0]["subjectId"], "trending-1");
        assert_eq!(subjects[0]["__browse_rank"], 1.0);
    }

    #[test]
    fn popularity_does_not_alias_top_rated_group() {
        let payload = serde_json::json!({
            "items": [
                {
                    "title": "Top 20 Movies",
                    "subjects": [{"subjectId": "top-1", "title": "All-time favorite"}]
                },
                {
                    "title": "Most Watched",
                    "subjects": [{"subjectId": "watched-1", "title": "Most watched"}]
                }
            ]
        });

        let subjects = App::extract_browse_subjects(&payload, BrowsePreset::PopularDesc);
        assert_eq!(subjects.len(), 1);
        assert_eq!(subjects[0]["subjectId"], "watched-1");
        assert!(subjects[0].get("__browse_rank").is_none());
    }
}
