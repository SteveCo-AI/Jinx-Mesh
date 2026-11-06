//! Bounded graph activation: the shared traversal primitive that loads seed
//! and neighbor pages under the node budget, plus the checkpoint-budget and
//! source-match helpers built on it.
use super::{ACTIVATION_BUDGET, ACTIVATION_PAGE};
use crate::{
    crypto::Key,
    graph,
    model::{ActivationStatus, GraphSlice, SourceMatch, State},
    store::{Store, TraversalCheckpoint},
};
use anyhow::Result;

pub(super) fn merge_slice(records: &mut GraphSlice, page: GraphSlice, budget: usize) {
    let mut added = 0usize;
    for (id, node) in page.nodes {
        if records.nodes.contains_key(&id) {
            records.nodes.insert(id.clone(), node);
            if let Some(edges) = page.edges.get(&id) {
                records.edges.insert(id, edges.clone());
            }
        } else if records.nodes.len() < budget && added < budget {
            records.nodes.insert(id.clone(), node);
            if let Some(edges) = page.edges.get(&id) {
                records.edges.insert(id, edges.clone());
            }
            added += 1;
        }
    }
}

// The callback and bounded frontier are kept explicit at this internal I/O boundary.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
pub(super) fn activate_graph(
    store: &Store,
    key: &Key,
    _metadata: &State,
    query: &str,
    seed_tokens: Option<&[String]>,
    resume: Option<&TraversalCheckpoint>,
    progress: &mut dyn FnMut(&ActivationStatus),
    page_limit: usize,
) -> Result<(
    GraphSlice,
    ActivationStatus,
    Vec<String>,
    Vec<String>,
    Vec<serde_json::Value>,
)> {
    let mut records = GraphSlice::default();
    let mut pages_loaded = 0usize;
    let mut visited = std::collections::BTreeSet::new();
    let mut frontier = std::collections::VecDeque::new();
    if let Some(checkpoint) = resume {
        visited.extend(checkpoint.visited.iter().cloned());
        frontier.extend(checkpoint.frontier.iter().cloned());
    }

    // Rehydrate only the query seeds and the prior provisional path. Processed
    // history stays as compact IDs in the checkpoint; it never consumes the
    // next invocation's node budget.
    let mut anchors = Vec::new();
    let input_tokens = seed_tokens
        .map(<[String]>::to_vec)
        .unwrap_or_else(|| graph::tokens(query));
    for token in input_tokens.into_iter().take(ACTIVATION_BUDGET) {
        let id = format!("lex:{token}");
        if !anchors.contains(&id) {
            anchors.push(id);
        }
    }
    let lexical_anchors = anchors.clone();
    if let Some(checkpoint) = resume {
        for token in graph::tokens(&checkpoint.provisional).into_iter().take(32) {
            let id = format!("lex:{token}");
            if !anchors.contains(&id) {
                anchors.push(id);
            }
        }
        frontier.extend(anchors.iter().filter(|id| !visited.contains(*id)).cloned());
    } else {
        frontier.extend(anchors.iter().cloned());
    }
    for (chunk_index, chunk) in anchors.chunks(ACTIVATION_PAGE).enumerate() {
        if pages_loaded >= page_limit {
            let pending: Vec<String> = anchors
                .iter()
                .skip((chunk_index * ACTIVATION_PAGE).min(anchors.len()))
                .filter(|id| !visited.contains(*id) && !frontier.contains(*id))
                .cloned()
                .collect();
            frontier.extend(pending);
            break;
        }
        if records.nodes.len() >= ACTIVATION_BUDGET {
            break;
        }
        let page = store.graph_nodes(key, chunk)?;
        pages_loaded += 1;
        merge_slice(&mut records, page, ACTIVATION_BUDGET);
        progress(&ActivationStatus {
            phase: "provisional".into(),
            nodes_loaded: records.nodes.len(),
            node_budget: ACTIVATION_BUDGET,
            pages_loaded,
            source_refs_seen: count_source_refs(&records),
            sources_loaded: 0,
            frontier_remaining: frontier.len(),
            can_continue: true,
            source_matches: Vec::new(),
        });
    }

    // Resolve only missing canonical words through old punctuation spellings.
    // This is optional compatibility I/O inside the same page budget.
    let aliases: Vec<String> = lexical_anchors
        .iter()
        .filter(|id| !records.nodes.contains_key(*id))
        .flat_map(|id| {
            [".", ",", "!", "?", ":", ";", "…"]
                .into_iter()
                .map(move |suffix| format!("{id}{suffix}"))
        })
        .take(ACTIVATION_BUDGET)
        .collect();
    for chunk in aliases.chunks(ACTIVATION_PAGE) {
        if pages_loaded >= page_limit {
            break;
        }
        let page = store.graph_nodes(key, chunk)?;
        pages_loaded += 1;
        frontier.extend(
            page.nodes
                .keys()
                .filter(|id| !frontier.contains(*id))
                .cloned()
                .collect::<Vec<_>>(),
        );
        merge_slice(&mut records, page, ACTIVATION_BUDGET);
    }
    // A missing keyed record was checked and is genuinely absent, so it does
    // not need to occupy the continuation frontier. It may be learned as a new
    // node later in this message.
    let missing_anchors: Vec<String> = anchors
        .iter()
        .filter(|id| !records.nodes.contains_key(*id))
        .cloned()
        .collect();
    visited.extend(missing_anchors.iter().cloned());
    frontier.retain(|id| !missing_anchors.contains(id));

    let reserved_unknown = lexical_anchors
        .iter()
        .filter(|id| {
            !records.nodes.contains_key(*id)
                && !records.nodes.values().any(|n| {
                    graph::tokens(&n.label).first()
                        == id.strip_prefix("lex:").map(str::to_owned).as_ref()
                })
        })
        .count();
    let activation_cap = ACTIVATION_BUDGET.saturating_sub(reserved_unknown);
    while !frontier.is_empty() && records.nodes.len() < activation_cap && pages_loaded < page_limit
    {
        let remaining = activation_cap - records.nodes.len();
        let batch_limit = remaining.clamp(1, ACTIVATION_PAGE / 2);
        let mut batch = Vec::with_capacity(batch_limit);
        while batch.len() < batch_limit {
            let Some(id) = frontier.pop_front() else {
                break;
            };
            if !visited.contains(&id) && !batch.contains(&id) {
                batch.push(id);
            }
        }
        if batch.is_empty() {
            break;
        }
        if visited.len().saturating_add(batch.len()) > 2048 {
            for id in batch.into_iter().rev() {
                frontier.push_front(id);
            }
            break;
        }
        let page = store.graph_neighbors(key, &batch, remaining.min(ACTIVATION_PAGE))?;
        pages_loaded += 1;
        let loaded_ids: Vec<String> = page.nodes.keys().cloned().collect();
        merge_slice(&mut records, page, ACTIVATION_BUDGET);
        visited.extend(batch);
        for id in loaded_ids {
            // Newly loaded nodes are queued even when their record was returned
            // as a neighbor: the next page must inspect their outgoing links.
            if let Some(edges) = records.edges.get(&id) {
                for target in edges.keys() {
                    if !visited.contains(target) && !frontier.contains(target) {
                        frontier.push_back(target.clone());
                    }
                }
            }
        }
        let provisional = ActivationStatus {
            phase: if frontier.is_empty() {
                "complete"
            } else {
                "provisional"
            }
            .into(),
            nodes_loaded: records.nodes.len(),
            node_budget: ACTIVATION_BUDGET,
            pages_loaded,
            source_refs_seen: count_source_refs(&records),
            sources_loaded: 0,
            frontier_remaining: frontier.len(),
            can_continue: true,
            source_matches: Vec::new(),
        };
        progress(&provisional);
    }

    let phase = if frontier.is_empty() {
        "complete"
    } else {
        "budget_limited"
    };
    let source_refs_seen = count_source_refs(&records);
    let interaction_ids = records
        .nodes
        .values()
        .filter_map(|node| {
            node.extra
                .get("source_episode_ids")
                .and_then(serde_json::Value::as_array)
        })
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .filter_map(|id| id.strip_prefix("episode:")?.parse::<u64>().ok())
        .collect::<std::collections::BTreeSet<_>>();
    let archive_ids: Vec<u64> = interaction_ids
        .into_iter()
        .flat_map(|user_event| [user_event, user_event.saturating_add(1)])
        .filter(|id| *id > 0)
        .take(32)
        .collect();
    let episodes = store.archive_by_ids(key, &archive_ids)?;
    let status = ActivationStatus {
        phase: phase.into(),
        nodes_loaded: records.nodes.len(),
        node_budget: ACTIVATION_BUDGET,
        pages_loaded,
        source_refs_seen,
        sources_loaded: episodes.len(),
        frontier_remaining: frontier.len(),
        can_continue: !frontier.is_empty()
            && anchors.len() < ACTIVATION_BUDGET
            && visited.len() < 2048
            && checkpoint_size_ok(query, &frontier, &visited),
        source_matches: source_matches(query, &episodes),
    };
    progress(&status);
    Ok((
        records,
        status,
        frontier.into_iter().collect(),
        visited.into_iter().collect(),
        episodes,
    ))
}

fn count_source_refs(records: &GraphSlice) -> usize {
    records
        .nodes
        .values()
        .filter_map(|node| {
            node.extra
                .get("source_episode_ids")
                .and_then(serde_json::Value::as_array)
        })
        .map(Vec::len)
        .sum()
}

fn checkpoint_size_ok(
    query: &str,
    frontier: &std::collections::VecDeque<String>,
    visited: &std::collections::BTreeSet<String>,
) -> bool {
    // The encrypted checkpoint has a fixed 32 KiB storage limit.
    query.len()
        + frontier.iter().map(String::len).sum::<usize>()
        + visited.iter().map(String::len).sum::<usize>()
        < 24 * 1024
}

pub(super) fn checkpoint_fits(checkpoint: &TraversalCheckpoint) -> bool {
    checkpoint.visited.len() <= 2048
        && checkpoint.frontier.len() <= 128
        && checkpoint_size_ok(
            &checkpoint.query,
            &checkpoint.frontier.iter().cloned().collect(),
            &checkpoint.visited.iter().cloned().collect(),
        )
}

pub(super) fn source_matches(query: &str, episodes: &[serde_json::Value]) -> Vec<SourceMatch> {
    let query_set: std::collections::BTreeSet<_> = graph::tokens(query).into_iter().collect();
    let mut matches = Vec::new();
    for episode in episodes {
        let (Some(id), Some(speaker), Some(text)) = (
            episode["id"].as_str(),
            episode["speaker"].as_str(),
            episode["text"].as_str(),
        ) else {
            continue;
        };
        let token_overlap = graph::tokens(text)
            .iter()
            .filter(|token| query_set.contains(*token))
            .count();
        if token_overlap > 0 {
            matches.push(SourceMatch {
                id: id.to_owned(),
                speaker: speaker.to_owned(),
                unix_time: episode["unix_time"].as_u64().unwrap_or(0),
                token_overlap,
            });
        }
    }
    matches.sort_by(|a, b| {
        b.token_overlap
            .cmp(&a.token_overlap)
            .then(b.unix_time.cmp(&a.unix_time))
    });
    matches.truncate(8);
    matches
}
