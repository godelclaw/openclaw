use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

pub type MemoryId = u64;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum MemoryTier {
    Public,
    Family,
    Private,
}

impl MemoryTier {
    #[must_use]
    pub fn rank(self) -> u8 {
        match self {
            Self::Public => 0,
            Self::Family => 1,
            Self::Private => 2,
        }
    }

    #[must_use]
    pub fn can_access(self, item_tier: MemoryTier) -> bool {
        self.rank() >= item_tier.rank()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum MemoryType {
    Fact,
    Preference,
    ProjectState,
    Relationship,
    Event,
    ExplicitRemember,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryItem {
    pub id: MemoryId,
    pub tier: MemoryTier,
    pub memory_type: MemoryType,
    pub summary: String,
    pub categories: Vec<String>,
    pub source_session: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub reinforcement_count: u32,
}

#[derive(Debug, Clone)]
pub struct CreateMemoryInput {
    pub tier: MemoryTier,
    pub memory_type: MemoryType,
    pub summary: String,
    pub categories: Vec<String>,
    pub source_session: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnEvent {
    pub session_key: String,
    pub timestamp: i64,
    pub user_text: String,
    pub assistant_text: String,
    pub tier: MemoryTier,
}

#[derive(Debug, Clone)]
pub struct SearchHit {
    pub item: MemoryItem,
    pub score: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CortexStats {
    pub total_items: usize,
    pub by_tier: HashMap<MemoryTier, usize>,
    pub by_type: HashMap<MemoryType, usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryCortex {
    next_id: MemoryId,
    items: HashMap<MemoryId, MemoryItem>,
    #[serde(skip)]
    category_index: HashMap<String, Vec<MemoryId>>,
}

impl Default for MemoryCortex {
    fn default() -> Self {
        Self {
            next_id: 1,
            items: HashMap::new(),
            category_index: HashMap::new(),
        }
    }
}

impl MemoryCortex {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn stats(&self) -> CortexStats {
        let mut by_tier = HashMap::new();
        let mut by_type = HashMap::new();

        for item in self.items.values() {
            *by_tier.entry(item.tier).or_insert(0) += 1;
            *by_type.entry(item.memory_type.clone()).or_insert(0) += 1;
        }

        CortexStats {
            total_items: self.items.len(),
            by_tier,
            by_type,
        }
    }

    pub fn remember(&mut self, input: CreateMemoryInput) -> MemoryId {
        self.remember_at(input, now_ts())
    }

    pub fn remember_at(&mut self, input: CreateMemoryInput, ts: i64) -> MemoryId {
        let id = self.next_id;
        self.next_id += 1;

        let normalized_categories = normalize_categories(input.categories);
        let item = MemoryItem {
            id,
            tier: input.tier,
            memory_type: input.memory_type,
            summary: input.summary.trim().to_string(),
            categories: normalized_categories.clone(),
            source_session: input.source_session,
            created_at: ts,
            updated_at: ts,
            reinforcement_count: 1,
        };

        self.items.insert(id, item);
        self.index_categories(id, &normalized_categories);
        id
    }

    pub fn reinforce(&mut self, id: MemoryId, ts: i64) -> bool {
        if let Some(item) = self.items.get_mut(&id) {
            item.reinforcement_count = item.reinforcement_count.saturating_add(1);
            item.updated_at = ts;
            return true;
        }
        false
    }

    #[must_use]
    pub fn get(&self, id: MemoryId) -> Option<&MemoryItem> {
        self.items.get(&id)
    }

    #[must_use]
    pub fn hot_items(
        &self,
        requesting_tier: MemoryTier,
        min_reinforcement: u32,
        limit: usize,
    ) -> Vec<MemoryItem> {
        let mut out: Vec<MemoryItem> = self
            .items
            .values()
            .filter(|item| requesting_tier.can_access(item.tier))
            .filter(|item| item.reinforcement_count >= min_reinforcement)
            .cloned()
            .collect();

        out.sort_by(|a, b| {
            b.reinforcement_count
                .cmp(&a.reinforcement_count)
                .then_with(|| b.updated_at.cmp(&a.updated_at))
        });
        out.truncate(limit);
        out
    }

    #[must_use]
    pub fn search(&self, query: &str, requesting_tier: MemoryTier, limit: usize) -> Vec<SearchHit> {
        let query_tokens = tokenize(query);
        if query_tokens.is_empty() {
            return Vec::new();
        }

        let mut hits: Vec<SearchHit> = self
            .items
            .values()
            .filter(|item| requesting_tier.can_access(item.tier))
            .filter_map(|item| {
                let score = score_item(item, &query_tokens);
                (score > 0.0).then(|| SearchHit {
                    item: item.clone(),
                    score,
                })
            })
            .collect();

        hits.sort_by(|a, b| {
            cmp_f32_desc(b.score, a.score).then_with(|| b.item.updated_at.cmp(&a.item.updated_at))
        });
        hits.truncate(limit);
        hits
    }

    /// memU-style ingestion: extract memory candidates from a turn without relying on explicit tool calls.
    pub fn ingest_turn(&mut self, event: TurnEvent) -> Vec<MemoryId> {
        let mut created = Vec::new();
        let lower = event.user_text.to_lowercase();

        if lower.contains("remember") {
            created.push(self.remember_at(
                CreateMemoryInput {
                    tier: event.tier,
                    memory_type: MemoryType::ExplicitRemember,
                    summary: event.user_text.trim().to_string(),
                    categories: vec!["explicit-remember".to_string(), "conversation".to_string()],
                    source_session: Some(event.session_key.clone()),
                },
                event.timestamp,
            ));
        }

        if let Some(pref) = extract_after_prefix(&event.user_text, "I prefer") {
            created.push(self.remember_at(
                CreateMemoryInput {
                    tier: event.tier,
                    memory_type: MemoryType::Preference,
                    summary: format!("User preference: {}", pref),
                    categories: vec!["preferences".to_string()],
                    source_session: Some(event.session_key.clone()),
                },
                event.timestamp,
            ));
        }

        if let Some(name) = extract_after_prefix(&event.user_text, "My name is") {
            created.push(self.remember_at(
                CreateMemoryInput {
                    tier: event.tier,
                    memory_type: MemoryType::Fact,
                    summary: format!("User stated name: {}", name),
                    categories: vec!["identity".to_string()],
                    source_session: Some(event.session_key),
                },
                event.timestamp,
            ));
        }

        created
    }

    pub fn save_json(&self, path: impl AsRef<Path>) -> Result<(), String> {
        let text = serde_json::to_string_pretty(self)
            .map_err(|err| format!("serialize memory cortex: {err}"))?;
        fs::write(path.as_ref(), text)
            .map_err(|err| format!("write memory cortex '{}': {err}", path.as_ref().display()))
    }

    pub fn load_json(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = path.as_ref();
        let text = fs::read_to_string(path)
            .map_err(|err| format!("read memory cortex '{}': {err}", path.display()))?;
        let mut cortex: MemoryCortex =
            serde_json::from_str(&text).map_err(|err| format!("parse memory cortex: {err}"))?;
        cortex.rebuild_indexes();
        Ok(cortex)
    }

    fn rebuild_indexes(&mut self) {
        self.category_index.clear();
        let entries: Vec<(MemoryId, Vec<String>)> = self
            .items
            .iter()
            .map(|(id, item)| (*id, item.categories.clone()))
            .collect();
        for (id, categories) in entries {
            self.index_categories(id, &categories);
            self.next_id = self.next_id.max(id.saturating_add(1));
        }
    }

    fn index_categories(&mut self, id: MemoryId, categories: &[String]) {
        for category in categories {
            self.category_index
                .entry(category.clone())
                .or_default()
                .push(id);
        }
    }
}

fn normalize_categories(categories: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for category in categories {
        let normalized = category.trim().to_lowercase();
        if !normalized.is_empty() && seen.insert(normalized.clone()) {
            out.push(normalized);
        }
    }
    out
}

fn tokenize(text: &str) -> HashSet<String> {
    text.split(|ch: char| !ch.is_alphanumeric())
        .map(|token| token.trim().to_lowercase())
        .filter(|token| !token.is_empty())
        .collect()
}

fn score_item(item: &MemoryItem, query_tokens: &HashSet<String>) -> f32 {
    let mut score = 0.0;
    let mut matched = false;
    let haystack_tokens = tokenize(&item.summary);

    for token in query_tokens {
        if haystack_tokens.contains(token) {
            matched = true;
            score += 2.0;
        }
        if item
            .categories
            .iter()
            .any(|category| category.contains(token.as_str()))
        {
            matched = true;
            score += 1.0;
        }
    }

    if !matched {
        return 0.0;
    }

    score + (item.reinforcement_count as f32 * 0.15)
}

fn extract_after_prefix(text: &str, prefix: &str) -> Option<String> {
    let idx = text.find(prefix)?;
    let start = idx + prefix.len();
    let tail = text[start..].trim_start();
    let cut_idx = tail.find(&['.', '!', '?', '\n'][..]).unwrap_or(tail.len());
    let value = tail[..cut_idx].trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn now_ts() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_secs() as i64,
        Err(_) => 0,
    }
}

fn cmp_f32_desc(a: f32, b: f32) -> Ordering {
    a.partial_cmp(&b).unwrap_or(Ordering::Equal)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tier_access_blocks_private_from_public_queries() {
        let mut cortex = MemoryCortex::new();
        cortex.remember_at(
            CreateMemoryInput {
                tier: MemoryTier::Public,
                memory_type: MemoryType::Fact,
                summary: "Moltbook is public-facing".to_string(),
                categories: vec!["moltbook".to_string()],
                source_session: None,
            },
            10,
        );
        cortex.remember_at(
            CreateMemoryInput {
                tier: MemoryTier::Private,
                memory_type: MemoryType::Fact,
                summary: "Proofgold private key is stored offline".to_string(),
                categories: vec!["proofgold".to_string()],
                source_session: None,
            },
            10,
        );

        let public_hits = cortex.search("proofgold", MemoryTier::Public, 10);
        let private_hits = cortex.search("proofgold", MemoryTier::Private, 10);

        assert_eq!(public_hits.len(), 0);
        assert_eq!(private_hits.len(), 1);
    }

    #[test]
    fn reinforcement_increases_rank() {
        let mut cortex = MemoryCortex::new();
        let a = cortex.remember_at(
            CreateMemoryInput {
                tier: MemoryTier::Private,
                memory_type: MemoryType::ProjectState,
                summary: "Vericore driver mode is active".to_string(),
                categories: vec!["vericore".to_string()],
                source_session: None,
            },
            100,
        );
        let _b = cortex.remember_at(
            CreateMemoryInput {
                tier: MemoryTier::Private,
                memory_type: MemoryType::ProjectState,
                summary: "Vericore gate mode was tested earlier".to_string(),
                categories: vec!["vericore".to_string()],
                source_session: None,
            },
            100,
        );

        let _ = cortex.reinforce(a, 101);
        let _ = cortex.reinforce(a, 102);

        let hits = cortex.search("vericore mode", MemoryTier::Private, 10);
        assert_eq!(hits.first().map(|hit| hit.item.id), Some(a));
    }

    #[test]
    fn ingest_turn_extracts_candidates() {
        let mut cortex = MemoryCortex::new();
        let ids = cortex.ingest_turn(TurnEvent {
            session_key: "telegram:181832275".to_string(),
            timestamp: 200,
            user_text:
                "I prefer concise replies. Remember that I am testing the rust memory cortex."
                    .to_string(),
            assistant_text: "Noted".to_string(),
            tier: MemoryTier::Private,
        });

        assert!(ids.len() >= 2);
        let hits = cortex.search("concise", MemoryTier::Private, 10);
        assert!(!hits.is_empty());
    }

    #[test]
    fn json_round_trip_rebuilds_indexes() {
        let mut cortex = MemoryCortex::new();
        let id = cortex.remember_at(
            CreateMemoryInput {
                tier: MemoryTier::Family,
                memory_type: MemoryType::Event,
                summary: "Family channel discussed travel plans".to_string(),
                categories: vec!["family".to_string(), "travel".to_string()],
                source_session: Some("telegram:-100family".to_string()),
            },
            300,
        );

        let tmp = std::env::temp_dir().join("adclaw-memory-test.json");
        cortex.save_json(&tmp).expect("save should work");
        let loaded = MemoryCortex::load_json(&tmp).expect("load should work");
        let _ = std::fs::remove_file(&tmp);

        assert!(loaded.get(id).is_some());
        let hits = loaded.search("travel", MemoryTier::Family, 10);
        assert_eq!(hits.first().map(|hit| hit.item.id), Some(id));
    }
}
