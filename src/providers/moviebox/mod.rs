pub mod adapt;
pub mod client;
pub mod crypto;
pub mod title;

pub use title::clean_moviebox_title;

use crate::providers::Provider;

impl Provider for client::MovieBoxClient {
    async fn search(&self, query: &str, page: usize) -> Result<serde_json::Value, String> {
        self.search(query, page)
            .await
            .map_err(|error| format!("{error:?}"))
    }

    async fn details(&self, id: &str) -> Result<serde_json::Value, String> {
        self.get_details(id)
            .await
            .map_err(|error| format!("{error:?}"))
    }
}

use client::{MovieBoxClient, ScraperError};
use serde_json::{Value, json};

const RESOURCE_PAGE_SIZE: usize = 20;

fn resource_page_path(
    subject_id: &str,
    season: usize,
    episode: usize,
    page: usize,
    resolution: Option<&str>,
) -> String {
    let res_param = resolution
        .filter(|value| !value.is_empty())
        .map(|value| format!("&resolution={value}"))
        .unwrap_or_default();

    if season == 0 && episode == 0 {
        format!(
            "/wefeed-mobile-bff/subject-api/resource?subjectId={subject_id}&page={page}&perPage={RESOURCE_PAGE_SIZE}{res_param}"
        )
    } else {
        format!(
            "/wefeed-mobile-bff/subject-api/resource?subjectId={subject_id}&se={season}&ep={episode}&page={page}&perPage={RESOURCE_PAGE_SIZE}{res_param}"
        )
    }
}

impl MovieBoxClient {
    pub async fn search(&self, query: &str, page: usize) -> Result<Value, ScraperError> {
        let payload = json!({
            "keyword": query,
            "page": page,
            "perPage": 20,
            "subjectType": "All",
            "tabId": "All"
        });
        self.post("/wefeed-mobile-bff/subject-api/search/v2", &payload)
            .await
    }

    pub async fn suggest(&self, query: &str) -> Result<Value, ScraperError> {
        self.search(query, 1).await
    }

    pub async fn get_details(&self, subject_id: &str) -> Result<Value, ScraperError> {
        let path = format!(
            "/wefeed-mobile-bff/subject-api/get?subjectId={}",
            subject_id
        );
        let mut details = self.get(&path).await?;

        let stype = details
            .get("subjectType")
            .and_then(|s| s.as_i64())
            .or_else(|| details.get("stype").and_then(|s| s.as_i64()))
            .unwrap_or(1);

        if stype == 2 {
            let season_path = format!(
                "/wefeed-mobile-bff/subject-api/season-info?subjectId={}",
                subject_id
            );
            if let Ok(season_info) = self.get(&season_path).await {
                if let Value::Object(ref mut map) = details {
                    map.insert("seasons".to_string(), season_info);
                }
            }
        }

        Ok(details)
    }

    pub async fn get_homepage(&self, tab_id: &str, page: usize) -> Result<Value, ScraperError> {
        let path = format!(
            "/wefeed-mobile-bff/tab-operating?page={}&tabId={}&version=",
            page, tab_id
        );
        self.get(&path).await
    }

    pub async fn get_resources(
        &self,
        subject_id: &str,
        season: usize,
        episode: usize,
        page: usize,
        resolution: Option<&str>,
    ) -> Result<Value, ScraperError> {
        let path = resource_page_path(subject_id, season, episode, page, resolution);
        self.get(&path).await
    }

    pub async fn fetch_resource_page(
        &self,
        subject_id: &str,
        resolution: u32,
        page: usize,
    ) -> Result<(Vec<Value>, Value), ScraperError> {
        let res_param = if resolution == 0 {
            String::new()
        } else {
            format!("&resolution={}", resolution)
        };

        let path = format!(
            "/wefeed-mobile-bff/subject-api/resource?subjectId={}&page={}&perPage=20{}",
            subject_id, page, res_param
        );

        let res = self.get(&path).await?;

        let items = res
            .get("list")
            .and_then(|l| l.as_array())
            .cloned()
            .unwrap_or_default();

        let pager = res.get("pager").cloned().unwrap_or_else(|| json!({}));

        Ok((items, pager))
    }

    pub async fn fetch_collection_resolutions(
        &self,
        subject_id: &str,
    ) -> Result<Vec<u32>, ScraperError> {
        let path = format!(
            "/wefeed-mobile-bff/subject-api/resource?subjectId={}&page=1&perPage=20",
            subject_id
        );
        let res = self.get(&path).await?;

        let mut resolutions = Vec::new();
        if let Some(cols) = res.get("collectionResolutions").and_then(|c| c.as_array()) {
            for col in cols {
                if let Some(r) = col.get("resolution").and_then(|v| v.as_u64()) {
                    resolutions.push(r as u32);
                }
            }
        }

        resolutions.sort_by(|a, b| b.cmp(a));

        if resolutions.is_empty() {
            resolutions = vec![1080, 720, 480, 360];
        }

        Ok(resolutions)
    }

    pub async fn get_ext_captions(
        &self,
        subject_id: &str,
        resource_id: &str,
    ) -> Result<Value, ScraperError> {
        let path = format!(
            "/wefeed-mobile-bff/subject-api/get-ext-captions?subjectId={}&resourceId={}",
            subject_id, resource_id
        );
        self.get(&path).await
    }
}

#[cfg(test)]
mod tests {
    use super::resource_page_path;

    #[test]
    fn resource_page_path_uses_provider_supported_page_size() {
        assert_eq!(
            resource_page_path("subject-fixture", 0, 0, 1, None),
            "/wefeed-mobile-bff/subject-api/resource?subjectId=subject-fixture&page=1&perPage=20"
        );
    }
}
