use anyhow::{Context, Result};
use yrs::{updates::decoder::Decode, Doc, GetString, ReadTxn, StateVector, Text, Transact, Update};

use crate::differ::{compute_diff, EditKind};

pub struct CrdtEngine {
    doc: Doc,
    text: yrs::TextRef,
    /// Last-known plaintext content (mirrors the Y.Text state).
    pub snapshot: String,
}

impl CrdtEngine {
    pub fn new() -> Self {
        let doc = Doc::new();
        let text = doc.get_or_insert_text("content");
        CrdtEngine {
            doc,
            text,
            snapshot: String::new(),
        }
    }

    /// Seed the document with initial content (used by the share command).
    pub fn seed(&mut self, content: &str) {
        if content.is_empty() {
            return;
        }
        let mut txn = self.doc.transact_mut();
        self.text.insert(&mut txn, 0, content);
        self.snapshot = content.to_string();
    }

    /// Apply a local file change. Returns the encoded incremental update to
    /// broadcast, or `None` if the content didn't change.
    pub fn apply_local_edit(&mut self, new_content: &str) -> Option<Vec<u8>> {
        let ops = compute_diff(&self.snapshot, new_content);
        if ops.is_empty() {
            return None;
        }

        let mut txn = self.doc.transact_mut();
        for op in &ops {
            match op.kind {
                EditKind::Delete => self.text.remove_range(&mut txn, op.pos, op.len),
                EditKind::Insert => self.text.insert(&mut txn, op.pos, &op.content),
            }
        }
        let update = txn.encode_update_v1();
        drop(txn);

        self.snapshot = new_content.to_string();
        if update.is_empty() {
            None
        } else {
            Some(update)
        }
    }

    /// Apply a remote update received from the relay. Returns the new plaintext
    /// if it changed, `None` otherwise.
    pub fn apply_remote_update(&mut self, data: &[u8]) -> Result<Option<String>> {
        let update = Update::decode_v1(data).context("failed to decode remote update")?;
        let mut txn = self.doc.transact_mut();
        txn.apply_update(update)
            .context("failed to apply remote update")?;
        let new_text = self.text.get_string(&txn);
        drop(txn);

        if new_text != self.snapshot {
            self.snapshot = new_text.clone();
            Ok(Some(new_text))
        } else {
            Ok(None)
        }
    }

    /// Return our state vector encoded as bytes (for SyncStep1).
    pub fn state_vector_bytes(&self) -> Vec<u8> {
        use yrs::updates::encoder::Encode;
        self.doc.transact().state_vector().encode_v1()
    }

    /// Encode the full doc state as an update relative to `peer_sv_bytes`
    /// (for SyncStep2 reply).
    pub fn encode_state_as_update(&self, peer_sv_bytes: &[u8]) -> Vec<u8> {
        let sv = StateVector::decode_v1(peer_sv_bytes).unwrap_or_default();
        self.doc.transact().encode_diff_v1(&sv)
    }

    /// Current text content.
    pub fn current_text(&self) -> &str {
        &self.snapshot
    }
}

impl Default for CrdtEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_and_read() {
        let mut engine = CrdtEngine::new();
        engine.seed("hello world");
        assert_eq!(engine.current_text(), "hello world");
    }

    #[test]
    fn local_edit_produces_update() {
        let mut engine = CrdtEngine::new();
        engine.seed("hello");
        let update = engine.apply_local_edit("hello world");
        assert!(update.is_some());
        assert_eq!(engine.current_text(), "hello world");
    }

    #[test]
    fn identical_edit_produces_no_update() {
        let mut engine = CrdtEngine::new();
        engine.seed("hello");
        assert!(engine.apply_local_edit("hello").is_none());
    }

    #[test]
    fn remote_update_round_trip() {
        let mut a = CrdtEngine::new();
        let mut b = CrdtEngine::new();

        a.seed("hello");
        let sv_b = b.state_vector_bytes();
        // Encode A's full state as an update for B (who has nothing)
        let update = a.encode_state_as_update(&sv_b);

        let new_text = b.apply_remote_update(&update).unwrap();
        assert_eq!(new_text, Some("hello".to_string()));
        assert_eq!(b.current_text(), "hello");
    }

    #[test]
    fn two_peer_convergence() {
        let mut a = CrdtEngine::new();
        let mut b = CrdtEngine::new();

        // Sync A → B: A has "hello"
        a.seed("hello");
        let a_sv = a.state_vector_bytes();
        let b_sv = b.state_vector_bytes();

        // A sends its state to B
        let a_to_b = a.encode_state_as_update(&b_sv);
        b.apply_remote_update(&a_to_b).unwrap();

        // B sends its state back to A
        let b_to_a = b.encode_state_as_update(&a_sv);
        a.apply_remote_update(&b_to_a).unwrap();

        // Now both have "hello". A makes a local edit.
        let a_update = a.apply_local_edit("hello world").unwrap();

        // B applies A's incremental update
        let new_text = b.apply_remote_update(&a_update).unwrap();
        assert_eq!(new_text, Some("hello world".to_string()));
        assert_eq!(a.current_text(), b.current_text());
    }

    #[test]
    fn concurrent_edits_converge() {
        let mut a = CrdtEngine::new();
        let mut b = CrdtEngine::new();

        // Both start with "hello world"
        a.seed("hello world");
        let a_to_b = a.encode_state_as_update(&b.state_vector_bytes());
        b.apply_remote_update(&a_to_b).unwrap();

        // A inserts at start, B inserts at end - concurrently
        let a_update = a.apply_local_edit("!! hello world").unwrap();
        let b_update = b.apply_local_edit("hello world !!").unwrap();

        // Cross-apply
        a.apply_remote_update(&b_update).unwrap();
        b.apply_remote_update(&a_update).unwrap();

        // Both should converge to the same text (exact text depends on CRDT
        // merge, but they must be identical)
        assert_eq!(a.current_text(), b.current_text());
    }

    #[test]
    fn state_vector_sync_protocol() {
        let mut sharer = CrdtEngine::new();
        let mut joiner = CrdtEngine::new();

        sharer.seed("shared content");

        // Joiner sends SyncStep1 (empty SV)
        let joiner_sv = joiner.state_vector_bytes();

        // Sharer replies with SyncStep2
        let step2 = sharer.encode_state_as_update(&joiner_sv);
        let result = joiner.apply_remote_update(&step2).unwrap();
        assert_eq!(result, Some("shared content".to_string()));
    }
}
