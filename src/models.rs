use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Indexer {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub url: String,
    pub api_key: String,
    pub cookie: String,
    pub enabled: bool,
    pub tags: Vec<String>,
    pub categories: Vec<u32>,
    pub request_delay_ms: u64,
    pub retries: u8,
    pub definition: Option<serde_json::Value>,
}
impl Default for Indexer {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            kind: "rss".into(),
            url: String::new(),
            api_key: String::new(),
            cookie: String::new(),
            enabled: true,
            tags: vec![],
            categories: vec![],
            request_delay_ms: 1000,
            retries: 0,
            definition: None,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Release {
    pub guid: String,
    pub title: String,
    pub link: String,
    pub details: Option<String>,
    pub indexer_id: String,
    pub indexer: String,
    pub size: Option<u64>,
    pub seeders: Option<u64>,
    pub peers: Option<u64>,
    pub published: Option<String>,
    pub categories: Vec<u32>,
    pub attributes: BTreeMap<String, String>,
    /// Identity of the engine runtime that produced this release; never serialized.
    /// Download tickets carry it so edited, deleted, or recreated sources reject old links.
    #[serde(skip)]
    pub source_epoch: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SearchQuery {
    #[serde(flatten)]
    pub params: BTreeMap<String, String>,
}
impl SearchQuery {
    pub fn get(&self, key: &str) -> &str {
        self.params.get(key).map(String::as_str).unwrap_or("")
    }
    pub fn keywords(&self) -> String {
        let mut words = vec![self.get("q").to_owned()];
        if !self.get("season").is_empty() {
            if let Ok(season) = self.get("season").parse::<u32>() {
                let mut episode = format!("S{season:02}");
                if let Ok(ep) = self.get("ep").parse::<u32>() {
                    episode.push_str(&format!("E{ep:02}"));
                }
                words.push(episode);
            }
        }
        for field in ["artist", "album", "title", "author", "publisher"] {
            if !self.get(field).is_empty() {
                words.push(self.get(field).to_owned());
            }
        }
        words.join(" ").trim().to_owned()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SearchError {
    pub indexer_id: String,
    pub indexer: String,
    pub message: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SearchResponse {
    pub results: Vec<Release>,
    pub errors: Vec<SearchError>,
    /// Sources still searching when `maxwait` elapsed; their results fill the cache for
    /// the next search instead of being thrown away.
    #[serde(default)]
    pub pending: Vec<SearchPending>,
    /// Sources not asked because their categories cannot carry the requested `cat`.
    #[serde(default)]
    pub skipped: Vec<String>,
    pub elapsed_ms: u64,
    pub cached: usize,
    /// Of `cached`, how many answered from a result set older than the TTL.
    #[serde(default)]
    pub stale: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SearchPending {
    pub indexer_id: String,
    pub indexer: String,
}
