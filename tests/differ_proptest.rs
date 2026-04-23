//! Property-based tests for the differ + CRDT round-trip.
//!
//! For any pair of UTF-8 strings, applying compute_diff(old, new) to a
//! Y.Text containing `old` must yield exactly `new`.

use entangle::crdt::CrdtEngine;
use entangle::differ::compute_diff;
use proptest::prelude::*;

/// Apply differ ops to a plain Vec<char> (mirrors what the CRDT does).
fn apply_ops(base: &str, ops: &[entangle::differ::EditOp]) -> String {
    use entangle::differ::EditKind;
    let mut chars: Vec<char> = base.chars().collect();
    for op in ops {
        let pos = op.pos as usize;
        match op.kind {
            EditKind::Delete => {
                let len = op.len as usize;
                assert!(
                    pos + len <= chars.len(),
                    "delete out of bounds: pos={pos} len={len} total={}",
                    chars.len()
                );
                chars.drain(pos..pos + len);
            }
            EditKind::Insert => {
                for (i, c) in op.content.chars().enumerate() {
                    chars.insert(pos + i, c);
                }
            }
        }
    }
    chars.into_iter().collect()
}

proptest! {
    #[test]
    fn diff_roundtrip_ascii(old in "[ -~]{0,200}", new in "[ -~]{0,200}") {
        let ops = compute_diff(&old, &new);
        let result = apply_ops(&old, &ops);
        prop_assert_eq!(result, new);
    }

    #[test]
    fn diff_roundtrip_unicode(
        old in "\\PC{0,100}",
        new in "\\PC{0,100}",
    ) {
        let ops = compute_diff(&old, &new);
        let result = apply_ops(&old, &ops);
        prop_assert_eq!(result, new);
    }

    #[test]
    fn crdt_roundtrip(old in "[ -~]{0,100}", new in "[ -~]{0,100}") {
        let mut engine = CrdtEngine::new();
        engine.seed(&old);
        let update = engine.apply_local_edit(&new);

        if old == new {
            prop_assert!(update.is_none());
        } else {
            prop_assert!(update.is_some());
        }
        prop_assert_eq!(engine.current_text(), new.as_str());
    }

    #[test]
    fn two_peer_convergence(
        initial in "[ -~]{0,80}",
        edit_a in "[ -~]{0,80}",
        edit_b in "[ -~]{0,80}",
    ) {
        // Both peers start with `initial`
        let mut a = CrdtEngine::new();
        let mut b = CrdtEngine::new();

        a.seed(&initial);
        let a_to_b = a.encode_state_as_update(&b.state_vector_bytes());
        b.apply_remote_update(&a_to_b).unwrap();

        // Make independent edits
        let update_a = a.apply_local_edit(&edit_a);
        let update_b = b.apply_local_edit(&edit_b);

        // Cross-apply
        if let Some(ua) = update_a {
            b.apply_remote_update(&ua).unwrap();
        }
        if let Some(ub) = update_b {
            a.apply_remote_update(&ub).unwrap();
        }

        // CRDTs must converge to the same text
        prop_assert_eq!(
            a.current_text(),
            b.current_text(),
            "peers diverged after concurrent edits"
        );
    }
}
