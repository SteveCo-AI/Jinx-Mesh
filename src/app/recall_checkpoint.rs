//! Archive recall and traversal continuation: bounded paged retrieval with
//! selectable strategies plus the idle prefetch step that advances a pending
//! checkpoint while the terminal waits for input.
use super::{
    activation::{activate_graph, checkpoint_fits, merge_slice},
    App,
};
use crate::{
    engine::Engine,
    graph,
    model::{ActivationStatus, GraphSlice},
    store::TraversalCheckpoint,
};
use anyhow::{bail, Result};

/// Bounded archive retrieval result. `complete` is false when the configured
/// page budget ended before the full archive was scanned.
#[derive(Clone, Debug)]
pub struct RecallResult {
    pub hits: Vec<serde_json::Value>,
    pub episodes_scanned: usize,
    pub total_episodes: u64,
    pub complete: bool,
    pub next_offset: Option<u64>,
}

impl App {
    /// Run one bounded, read-only continuation step while the terminal waits
    /// for user input. The periodic caller must use this only during the chat
    /// prompt, never while prompting for credentials.
    pub fn prefetch_idle_step(&mut self) -> Result<Option<ActivationStatus>> {
        self.tick();
        let Some(s) = self.session.as_mut().filter(|session| !session.frozen) else {
            return Ok(None);
        };
        let key = s.key.as_ref().expect("developer key");
        let Some(checkpoint) = self
            .store
            .load_checkpoint(key, "active", s.payload.sequence)?
        else {
            return Ok(None);
        };
        let metadata = s.payload.state.clone();
        let (records, activation, frontier, visited, _) = activate_graph(
            &self.store,
            key,
            &metadata,
            &checkpoint.query,
            None,
            Some(&checkpoint),
            &mut |_| {},
            1,
        )?;
        s.graph_cache = records;
        s.engine = Engine::new(
            Engine::active_state(&metadata, &s.graph_cache),
            s.payload.actions.clone(),
        )?;
        if frontier.is_empty() {
            let _ = self.store.clear_checkpoint(key, "active")?;
        } else {
            let next = TraversalCheckpoint {
                revision: s.payload.sequence,
                frontier,
                visited,
                provisional: checkpoint.provisional,
                query: checkpoint.query,
            };
            if activation.can_continue && checkpoint_fits(&next) {
                let _ = self.store.save_checkpoint(key, "active", &next)?;
            }
        }
        s.prefetch_status = Some(activation.clone());
        Ok(Some(activation))
    }

    /// Exact local recall from authenticated, encrypted conversation episodes.
    /// This is a bounded keyword retrieval primitive, not semantic understanding.
    pub fn recall(&mut self, query: &str, limit: usize) -> Result<Vec<serde_json::Value>> {
        self.recall_with_strategy(query, limit, None)
    }

    /// Run explicitly selected archive handlers. The default combines lexical
    /// and observed two-token context; temporal ordering is opt-in.
    pub fn recall_with_strategy(
        &mut self,
        query: &str,
        limit: usize,
        strategy: Option<&str>,
    ) -> Result<Vec<serde_json::Value>> {
        Ok(self
            .recall_with_strategy_progress(query, limit, strategy, |_, _| {})?
            .hits)
    }

    /// Search a fixed number of encrypted archive pages and report actual page
    /// progress. Context vectors are fetched from bounded learned graph pages.
    pub fn recall_with_strategy_progress<F>(
        &mut self,
        query: &str,
        limit: usize,
        strategy: Option<&str>,
        progress: F,
    ) -> Result<RecallResult>
    where
        F: FnMut(usize, usize),
    {
        self.recall_with_strategy_before_progress(query, limit, strategy, 0, progress)
    }

    /// Search older archive ranges by offset. The offset counts episodes back
    /// from the newest item and is returned as a continuation cursor.
    pub fn recall_with_strategy_before_progress<F>(
        &mut self,
        query: &str,
        limit: usize,
        strategy: Option<&str>,
        before: u64,
        mut progress: F,
    ) -> Result<RecallResult>
    where
        F: FnMut(usize, usize),
    {
        self.tick();
        let s = self
            .session
            .as_ref()
            .filter(|s| !s.frozen)
            .ok_or_else(|| anyhow::anyhow!("Developer-Anmeldung erforderlich"))?;
        if graph::tokens(query).is_empty() {
            return Ok(RecallResult {
                hits: Vec::new(),
                episodes_scanned: 0,
                total_episodes: 0,
                complete: true,
                next_offset: None,
            });
        }
        let key = s
            .key
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Anmeldung erforderlich"))?;
        let total = self.store.archive_len()?;
        const ARCHIVE_PAGE: usize = 32;
        const ARCHIVE_PAGE_BUDGET: usize = 128;
        const GRAPH_CONTEXT_BUDGET: usize = 128;
        let controller = crate::retrieval::RetrievalController::defaults()?;
        let selected: Vec<&str> = match strategy {
            Some(id) if controller.has(id) => vec![id],
            Some(id) => bail!("Unbekannte Retrieval-Strategie: {id}"),
            None => vec!["lexical", "contextual"],
        };
        let mut scored: Vec<crate::retrieval::Candidate> = Vec::new();
        let mut end = total.saturating_sub(before);
        let result_budget = limit.clamp(1, 20);
        let total_pages = end.div_ceil(ARCHIVE_PAGE as u64) as usize;
        let mut pages_scanned = 0usize;
        while end > 0 && pages_scanned < ARCHIVE_PAGE_BUDGET {
            let start = end.saturating_sub(ARCHIVE_PAGE as u64);
            let events = self
                .store
                .archive_page(key, start, (end - start) as usize)?;
            let mut context = GraphSlice::default();
            if selected.contains(&"contextual") {
                let query_seeds: Vec<String> = graph::tokens(query)
                    .into_iter()
                    .take(32)
                    .map(|token| format!("lex:{token}"))
                    .collect();
                let query_context =
                    self.store
                        .graph_neighbors(key, &query_seeds, GRAPH_CONTEXT_BUDGET / 2)?;
                merge_slice(&mut context, query_context, GRAPH_CONTEXT_BUDGET);
                let candidate_seeds: Vec<String> = events
                    .iter()
                    .filter_map(|event| event["text"].as_str())
                    .flat_map(graph::tokens)
                    .map(|token| format!("lex:{token}"))
                    .filter(|id| !context.nodes.contains_key(id))
                    .collect::<std::collections::BTreeSet<_>>()
                    .into_iter()
                    .take(32)
                    .collect();
                let candidate_context =
                    self.store
                        .graph_neighbors(key, &candidate_seeds, GRAPH_CONTEXT_BUDGET / 2)?;
                merge_slice(&mut context, candidate_context, GRAPH_CONTEXT_BUDGET);
            }
            for id in &selected {
                if !controller.has(id) {
                    bail!("Retrieval-Handler fehlt: {id}");
                }
                scored.extend(controller.get(id)?.search(
                    query,
                    &events,
                    result_budget * 3,
                    &context,
                ));
            }
            if strategy == Some("temporal") {
                scored.sort_by_key(|candidate| {
                    std::cmp::Reverse(candidate.episode["unix_time"].as_u64().unwrap_or(0))
                });
            } else {
                scored.sort_by_key(|candidate| std::cmp::Reverse(candidate.score));
            }
            scored.truncate(result_budget * 3);
            end = start;
            pages_scanned += 1;
            if pages_scanned.is_multiple_of(4) || end == 0 {
                progress(pages_scanned, total_pages);
            }
        }
        if strategy == Some("temporal") {
            scored.sort_by_key(|candidate| {
                std::cmp::Reverse(candidate.episode["unix_time"].as_u64().unwrap_or(0))
            });
        } else {
            scored.sort_by_key(|candidate| std::cmp::Reverse(candidate.score));
        }
        let mut seen = std::collections::BTreeSet::new();
        let hits = scored
            .into_iter()
            .filter(|candidate| {
                seen.insert(candidate.episode["id"].as_str().unwrap_or("").to_owned())
            })
            .take(result_budget)
            .map(|candidate| candidate.episode)
            .collect();
        let complete = end == 0;
        Ok(RecallResult {
            hits,
            episodes_scanned: (pages_scanned * ARCHIVE_PAGE)
                .min(total.saturating_sub(before) as usize),
            total_episodes: total,
            complete,
            next_offset: (!complete)
                .then_some(before.saturating_add((pages_scanned * ARCHIVE_PAGE) as u64)),
        })
    }
}
