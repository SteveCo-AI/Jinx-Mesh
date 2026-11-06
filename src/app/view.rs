//! Read-only views and integrity commands: resident graph view, /status,
//! and the full archive/graph verification streams.
use super::App;
use crate::journal;
use anyhow::Result;
use serde_json::json;

impl App {
    /// Bounded, read-only view of the currently resident network. No archive text.
    pub fn graph_view(&self) -> serde_json::Value {
        let Some(s) = self.session.as_ref() else {
            return json!({"locked": true, "nodes": [], "edges": []});
        };
        let nodes: Vec<_> = s
            .engine
            .state
            .neuronen
            .values()
            .take(128)
            .map(|n| json!({"id": n.id, "label": n.label, "strength": n.staerke}))
            .collect();
        let ids: std::collections::BTreeSet<_> =
            nodes.iter().filter_map(|n| n["id"].as_str()).collect();
        let edges: Vec<_> = s
            .engine
            .state
            .kanten
            .iter()
            .flat_map(|(a, targets)| targets.iter().map(move |(b, w)| (a, b, w)))
            .filter(|(a, b, _)| ids.contains(a.as_str()) && ids.contains(b.as_str()))
            .take(512)
            .map(|(a, b, w)| json!({"from": a, "to": b, "weight": w}))
            .collect();
        json!({"locked": false, "nodes": nodes, "edges": edges,
            "total": s.payload.graph_count, "status": s.prefetch_status})
    }

    pub fn status(&mut self) -> Result<serde_json::Value> {
        self.tick();
        let s = self
            .session
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Gesperrt"))?;
        // Crash recovery is reported once, honestly, at the first /status.
        let hinweis = (self.verworfen_ungemeldet > 0)
            .then(|| journal::verwurf_hinweis(self.verworfen_ungemeldet));
        self.verworfen_ungemeldet = 0;
        let retrieval = crate::retrieval::RetrievalController::defaults()?;
        let mut out = json!({"mode":if s.frozen{"frozen"}else{"developer"},"neurons":s.payload.graph_count,"active_nodes":s.graph_cache.nodes.len(),"active_edges":s.graph_cache.edges.values().map(|e|e.len()).sum::<usize>(),"interactions":s.engine.state.interaktionen,"prefetch":s.prefetch_status,"audit_entries":self.store.audit_len()?,"retrieval_handlers":retrieval.ordered_ids(),"feedback_counts":s.payload.state.extra.get("feedback_counts"),"feedback_available":s.feedback_target.is_some(),"journal_länge":s.journal.len(),"journal_letzter":s.journal.last().map(|e| json!({"art":e.art,"nummer":e.interaktionsnummer,"seq":e.seq}))});
        if let Some(hinweis) = hinweis {
            out["systemhinweis"] = json!(hinweis);
        }
        Ok(out)
    }

    /// Stream a complete integrity check of every archived conversation block.
    /// Normal login checks only the bounded manifest and indexed reads verify
    /// their own blocks; this command intentionally pays the full I/O cost.
    pub fn verify_archive_full(&mut self) -> Result<u64> {
        self.tick();
        let s = self
            .session
            .as_ref()
            .filter(|s| !s.frozen)
            .ok_or_else(|| anyhow::anyhow!("Developer-Anmeldung erforderlich"))?;
        self.store.verify_archive_full(
            s.key.as_ref().expect("developer key"),
            s.payload.archive_count,
            &s.payload.archive_root,
        )?;
        Ok(s.payload.archive_count)
    }

    /// Stream an authenticated integrity check of every encrypted graph record.
    pub fn verify_graph_full(&mut self) -> Result<u64> {
        self.tick();
        let s = self
            .session
            .as_ref()
            .filter(|s| !s.frozen)
            .ok_or_else(|| anyhow::anyhow!("Developer-Anmeldung erforderlich"))?;
        self.store.verify_graph_full(
            s.key.as_ref().expect("developer key"),
            s.payload.graph_count,
            &s.payload.graph_root,
        )?;
        Ok(s.payload.graph_count)
    }
}
