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
    TopSecret,
}

impl MemoryTier {
    #[must_use]
    pub fn rank(self) -> u8 {
        match self {
            Self::Public => 0,
            Self::Family => 1,
            Self::Private => 2,
            Self::TopSecret => 3,
        }
    }

    #[must_use]
    pub fn can_access(self, item_tier: MemoryTier) -> bool {
        match (self, item_tier) {
            (Self::TopSecret, _) => true,
            (Self::Private, Self::TopSecret) => true,
            _ => self.rank() >= item_tier.rank(),
        }
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_date: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub reinforcement_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding: Option<Vec<f32>>,
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
pub struct MemoryRefineReport {
    pub before_items: usize,
    pub after_items: usize,
    pub removed_items: usize,
    pub merged_groups: usize,
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
            source_date: None,
            created_at: ts,
            updated_at: ts,
            reinforcement_count: 1,
            embedding: None,
        };

        self.items.insert(id, item);
        self.index_categories(id, &normalized_categories);
        id
    }

    pub fn remember_or_reinforce(&mut self, input: CreateMemoryInput) -> (MemoryId, bool) {
        self.remember_or_reinforce_at(input, now_ts())
    }

    pub fn remember_or_reinforce_at(
        &mut self,
        input: CreateMemoryInput,
        ts: i64,
    ) -> (MemoryId, bool) {
        self.remember_or_reinforce_impl(input, ts, true)
    }

    pub fn remember_if_new_at(&mut self, input: CreateMemoryInput, ts: i64) -> (MemoryId, bool) {
        self.remember_or_reinforce_impl(input, ts, false)
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

    pub fn set_embedding(&mut self, id: MemoryId, embedding: Vec<f32>) -> bool {
        if let Some(item) = self.items.get_mut(&id) {
            item.embedding = Some(embedding);
            item.updated_at = now_ts();
            return true;
        }
        false
    }

    pub fn set_source_date(&mut self, id: MemoryId, source_date: Option<String>) -> bool {
        if let Some(item) = self.items.get_mut(&id) {
            item.source_date = source_date;
            return true;
        }
        false
    }

    pub fn set_tier(
        &mut self,
        id: MemoryId,
        new_tier: MemoryTier,
        operator_approved: bool,
    ) -> Result<bool, String> {
        let Some(item) = self.items.get_mut(&id) else {
            return Err(format!("memory id not found: {}", id));
        };

        let old_tier = item.tier;
        if old_tier == new_tier {
            return Ok(false);
        }

        let is_demotion = new_tier.rank() < old_tier.rank();
        if is_demotion && !operator_approved {
            if old_tier == MemoryTier::TopSecret {
                return Err("top_secret downgrade requires operator approval".to_string());
            }
            return Err("tier downgrade requires operator approval".to_string());
        }

        item.tier = new_tier;
        item.updated_at = now_ts();
        Ok(true)
    }

    #[must_use]
    pub fn ids_missing_embeddings(&self) -> Vec<MemoryId> {
        self.items
            .values()
            .filter(|item| item.embedding.is_none())
            .map(|item| item.id)
            .collect()
    }

    #[must_use]
    pub fn ids_missing_source_date(&self) -> Vec<MemoryId> {
        self.items
            .values()
            .filter(|item| item.source_date.is_none())
            .map(|item| item.id)
            .collect()
    }

    /// Deterministic offline maintenance pass.
    ///
    /// This is intended for low-frequency "sleep" windows. It does no network calls
    /// and only merges strongly equivalent memories (same tier/type/normalized summary).
    pub fn sleep_refine(&mut self) -> MemoryRefineReport {
        let before_items = self.items.len();
        let mut buckets: HashMap<(MemoryTier, MemoryType, String), Vec<MemoryId>> = HashMap::new();

        for item in self.items.values() {
            let norm = normalize_summary_for_dedupe(&item.summary);
            if norm.is_empty() {
                continue;
            }
            buckets
                .entry((item.tier, item.memory_type.clone(), norm))
                .or_default()
                .push(item.id);
        }

        let mut merge_pairs: Vec<(MemoryId, MemoryId)> = Vec::new();
        let mut merged_groups = 0usize;

        for mut ids in buckets.into_values() {
            if ids.len() <= 1 {
                continue;
            }

            ids.sort_by_key(|id| {
                self.items
                    .get(id)
                    .map(|item| {
                        (
                            std::cmp::Reverse(item.reinforcement_count),
                            std::cmp::Reverse(item.updated_at),
                            *id,
                        )
                    })
                    .unwrap_or((std::cmp::Reverse(0_u32), std::cmp::Reverse(0_i64), *id))
            });

            let canonical_id = ids[0];
            for duplicate_id in ids.into_iter().skip(1) {
                merge_pairs.push((canonical_id, duplicate_id));
            }
            merged_groups = merged_groups.saturating_add(1);
        }

        for (canonical_id, duplicate_id) in merge_pairs {
            let Some(duplicate) = self.items.remove(&duplicate_id) else {
                continue;
            };
            let Some(canonical) = self.items.get_mut(&canonical_id) else {
                continue;
            };

            canonical.reinforcement_count = canonical
                .reinforcement_count
                .saturating_add(duplicate.reinforcement_count);
            canonical.created_at = canonical.created_at.min(duplicate.created_at);
            canonical.updated_at = canonical.updated_at.max(duplicate.updated_at);

            if canonical.source_session.is_none() {
                canonical.source_session = duplicate.source_session.clone();
            }
            if canonical.source_date.is_none() {
                canonical.source_date = duplicate.source_date.clone();
            }
            if canonical.embedding.is_none() {
                canonical.embedding = duplicate.embedding.clone();
            }

            let mut categories = canonical.categories.clone();
            categories.extend(duplicate.categories.iter().cloned());
            canonical.categories = normalize_categories(categories);
        }

        self.rebuild_indexes();

        let after_items = self.items.len();
        MemoryRefineReport {
            before_items,
            after_items,
            removed_items: before_items.saturating_sub(after_items),
            merged_groups,
        }
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
        self.search_hybrid(query, None, requesting_tier, limit)
    }

    #[must_use]
    pub fn search_hybrid(
        &self,
        query: &str,
        query_embedding: Option<&[f32]>,
        requesting_tier: MemoryTier,
        limit: usize,
    ) -> Vec<SearchHit> {
        let query_tokens = tokenize(query);
        if query_tokens.is_empty() && query_embedding.is_none() {
            return Vec::new();
        }

        let mut hits: Vec<SearchHit> = self
            .items
            .values()
            .filter(|item| requesting_tier.can_access(item.tier))
            .filter_map(|item| {
                let lexical = lexical_score_item(item, &query_tokens);
                let semantic = semantic_score_item(item, query_embedding).unwrap_or(0.0);

                if lexical <= 0.0 && semantic <= 0.0 {
                    return None;
                }

                let score = hybrid_score(lexical, semantic, item.reinforcement_count);
                Some(SearchHit {
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
            let (id, was_created) = self.remember_or_reinforce_at(
                CreateMemoryInput {
                    tier: event.tier,
                    memory_type: MemoryType::ExplicitRemember,
                    summary: event.user_text.trim().to_string(),
                    categories: vec!["explicit-remember".to_string(), "conversation".to_string()],
                    source_session: Some(event.session_key.clone()),
                },
                event.timestamp,
            );
            if was_created {
                created.push(id);
            }
        }

        if let Some(pref) = extract_after_prefix(&event.user_text, "I prefer") {
            let (id, was_created) = self.remember_or_reinforce_at(
                CreateMemoryInput {
                    tier: event.tier,
                    memory_type: MemoryType::Preference,
                    summary: format!("User preference: {}", pref),
                    categories: vec!["preferences".to_string()],
                    source_session: Some(event.session_key.clone()),
                },
                event.timestamp,
            );
            if was_created {
                created.push(id);
            }
        }

        if let Some(name) = extract_after_prefix(&event.user_text, "My name is") {
            let (id, was_created) = self.remember_or_reinforce_at(
                CreateMemoryInput {
                    tier: event.tier,
                    memory_type: MemoryType::Fact,
                    summary: format!("User stated name: {}", name),
                    categories: vec!["identity".to_string()],
                    source_session: Some(event.session_key),
                },
                event.timestamp,
            );
            if was_created {
                created.push(id);
            }
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

    fn remember_or_reinforce_impl(
        &mut self,
        input: CreateMemoryInput,
        ts: i64,
        reinforce_on_duplicate: bool,
    ) -> (MemoryId, bool) {
        if let Some(existing_id) = self.find_duplicate_candidate(&input) {
            if reinforce_on_duplicate {
                if let Some(existing) = self.items.get_mut(&existing_id) {
                    existing.reinforcement_count = existing.reinforcement_count.saturating_add(1);
                    existing.updated_at = ts;
                    // Keep categories rich over time while preserving deterministic normalization.
                    let mut merged = existing.categories.clone();
                    merged.extend(input.categories.iter().cloned());
                    existing.categories = normalize_categories(merged);
                }
                self.rebuild_indexes();
            }
            return (existing_id, false);
        }

        (self.remember_at(input, ts), true)
    }

    fn find_duplicate_candidate(&self, input: &CreateMemoryInput) -> Option<MemoryId> {
        let input_norm = normalize_summary_for_dedupe(&input.summary);
        if input_norm.is_empty() {
            return None;
        }
        let input_tokens = tokenize(&input.summary);

        self.items
            .values()
            .filter(|item| item.tier == input.tier)
            .filter(|item| item.memory_type == input.memory_type)
            .filter_map(|item| {
                let existing_norm = normalize_summary_for_dedupe(&item.summary);
                if existing_norm == input_norm {
                    return Some((item.id, 1.0_f32, item.updated_at));
                }

                let existing_tokens = tokenize(&item.summary);
                let overlap = jaccard_similarity(&input_tokens, &existing_tokens);
                (overlap >= 0.92).then_some((item.id, overlap, item.updated_at))
            })
            .max_by(|a, b| cmp_f32_desc(a.1, b.1).then_with(|| a.2.cmp(&b.2)))
            .map(|(id, _, _)| id)
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

fn lexical_score_item(item: &MemoryItem, query_tokens: &HashSet<String>) -> f32 {
    if query_tokens.is_empty() {
        return 0.0;
    }

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

    score
}

fn semantic_score_item(item: &MemoryItem, query_embedding: Option<&[f32]>) -> Option<f32> {
    let query = query_embedding?;
    let item_embedding = item.embedding.as_ref()?;
    cosine_similarity(query, item_embedding)
}

fn hybrid_score(lexical: f32, semantic: f32, reinforcement_count: u32) -> f32 {
    let reinforcement = (reinforcement_count as f32 + 1.0).ln();
    lexical + (semantic.max(0.0) * 3.0) + (reinforcement * 0.4)
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> Option<f32> {
    if a.is_empty() || b.is_empty() || a.len() != b.len() {
        return None;
    }

    let mut dot = 0.0_f32;
    let mut na = 0.0_f32;
    let mut nb = 0.0_f32;
    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na <= 0.0 || nb <= 0.0 {
        return None;
    }
    Some(dot / (na.sqrt() * nb.sqrt()))
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

fn normalize_summary_for_dedupe(text: &str) -> String {
    let mut tokens: Vec<_> = tokenize(text).into_iter().collect();
    tokens.sort();
    tokens.join(" ")
}

fn jaccard_similarity(a: &HashSet<String>, b: &HashSet<String>) -> f32 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let intersection = a.intersection(b).count() as f32;
    let union = a.union(b).count() as f32;
    if union <= 0.0 {
        0.0
    } else {
        intersection / union
    }
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
    fn ingest_turn_dedupes_and_reinforces() {
        let mut cortex = MemoryCortex::new();
        let event = TurnEvent {
            session_key: "telegram:181832275".to_string(),
            timestamp: 400,
            user_text: "Remember: prioritize reliability over low-quality fallbacks.".to_string(),
            assistant_text: "Noted".to_string(),
            tier: MemoryTier::Private,
        };

        let created_1 = cortex.ingest_turn(event.clone());
        let created_2 = cortex.ingest_turn(event);

        assert_eq!(created_1.len(), 1);
        assert_eq!(created_2.len(), 0);

        let id = created_1[0];
        let item = cortex.get(id).expect("item should exist");
        assert!(item.reinforcement_count >= 2);
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

    #[test]
    fn sleep_refine_merges_exact_duplicates() {
        let mut cortex = MemoryCortex::new();
        let _a = cortex.remember_at(
            CreateMemoryInput {
                tier: MemoryTier::Private,
                memory_type: MemoryType::ProjectState,
                summary: "Weekly deploy preference remains reliability-first".to_string(),
                categories: vec!["deploy".to_string()],
                source_session: Some("telegram:1".to_string()),
            },
            100,
        );
        let _b = cortex.remember_at(
            CreateMemoryInput {
                tier: MemoryTier::Private,
                memory_type: MemoryType::ProjectState,
                summary: "weekly deploy preference remains reliability first".to_string(),
                categories: vec!["reliability".to_string()],
                source_session: Some("telegram:2".to_string()),
            },
            101,
        );

        let report = cortex.sleep_refine();
        assert_eq!(report.before_items, 2);
        assert_eq!(report.after_items, 1);
        assert_eq!(report.removed_items, 1);

        let hits = cortex.search("weekly deploy reliability", MemoryTier::Private, 5);
        assert_eq!(hits.len(), 1);
        assert!(hits[0].item.reinforcement_count >= 2);
        assert!(hits[0].item.categories.iter().any(|c| c == "deploy"));
        assert!(hits[0].item.categories.iter().any(|c| c == "reliability"));
    }

    #[test]
    fn ids_missing_source_date_reports_items_without_dates() {
        let mut cortex = MemoryCortex::new();
        let id = cortex.remember_at(
            CreateMemoryInput {
                tier: MemoryTier::Private,
                memory_type: MemoryType::Fact,
                summary: "User stated timezone: CET".to_string(),
                categories: vec!["profile".to_string()],
                source_session: None,
            },
            123,
        );

        let missing = cortex.ids_missing_source_date();
        assert_eq!(missing, vec![id]);

        assert!(cortex.set_source_date(id, Some("2026-02-28".to_string())));
        assert!(cortex.ids_missing_source_date().is_empty());
    }

    #[test]
    fn top_secret_downgrade_requires_operator_approval() {
        let mut cortex = MemoryCortex::new();
        let id = cortex.remember_at(
            CreateMemoryInput {
                tier: MemoryTier::Private,
                memory_type: MemoryType::Fact,
                summary: "Friend shared highly sensitive personal details".to_string(),
                categories: vec!["friends".to_string()],
                source_session: None,
            },
            100,
        );

        assert!(
            cortex
                .set_tier(id, MemoryTier::TopSecret, false)
                .expect("promotion should work")
        );

        let no_approval = cortex.set_tier(id, MemoryTier::Private, false);
        assert!(no_approval.is_err());

        let with_approval = cortex.set_tier(id, MemoryTier::Private, true);
        assert!(with_approval.expect("approved downgrade should work"));
    }

    #[test]
    fn private_queries_can_access_top_secret_items() {
        let mut cortex = MemoryCortex::new();
        cortex.remember_at(
            CreateMemoryInput {
                tier: MemoryTier::TopSecret,
                memory_type: MemoryType::Fact,
                summary: "Top secret memory artifact".to_string(),
                categories: vec!["top-secret".to_string()],
                source_session: None,
            },
            200,
        );

        let private_hits = cortex.search("artifact", MemoryTier::Private, 10);
        assert_eq!(private_hits.len(), 1);

        let public_hits = cortex.search("artifact", MemoryTier::Public, 10);
        assert!(public_hits.is_empty());
    }
}
