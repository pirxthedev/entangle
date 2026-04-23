use similar::{ChangeTag, TextDiff};

#[derive(Debug, Clone, PartialEq)]
pub enum EditKind {
    Insert,
    Delete,
}

#[derive(Debug, Clone)]
pub struct EditOp {
    pub kind: EditKind,
    /// Character position in the target text (Unicode scalar offsets, same as yrs).
    pub pos: u32,
    /// Text to insert (only set for Insert ops).
    pub content: String,
    /// Number of characters to delete (only set for Delete ops).
    pub len: u32,
}

/// Compute a sequence of insert/delete operations that transform `old` into `new`.
///
/// The ops are ordered so that they can be applied left-to-right to a Y.Text
/// containing `old`, yielding `new`. Positions are tracked in the "current CRDT
/// state" frame: deletes don't advance the cursor (they remove chars at the
/// cursor), while inserts advance it by the number of chars inserted.
pub fn compute_diff(old: &str, new: &str) -> Vec<EditOp> {
    if old == new {
        return vec![];
    }

    let diff = TextDiff::configure().diff_chars(old, new);
    let mut ops = Vec::new();
    let mut crdt_pos: u32 = 0;

    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Equal => {
                crdt_pos += change.value().chars().count() as u32;
            }
            ChangeTag::Delete => {
                let len = change.value().chars().count() as u32;
                ops.push(EditOp {
                    kind: EditKind::Delete,
                    pos: crdt_pos,
                    content: String::new(),
                    len,
                });
                // Deletions remove chars at crdt_pos; don't advance it.
            }
            ChangeTag::Insert => {
                let content = change.value().to_string();
                let char_len = content.chars().count() as u32;
                ops.push(EditOp {
                    kind: EditKind::Insert,
                    pos: crdt_pos,
                    content,
                    len: 0,
                });
                crdt_pos += char_len;
            }
        }
    }

    ops
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Apply ops to `base` string, returning the result.
    fn apply_ops(base: &str, ops: &[EditOp]) -> String {
        let mut chars: Vec<char> = base.chars().collect();
        for op in ops {
            let pos = op.pos as usize;
            match op.kind {
                EditKind::Delete => {
                    let len = op.len as usize;
                    chars.drain(pos..pos + len);
                }
                EditKind::Insert => {
                    let insert_chars: Vec<char> = op.content.chars().collect();
                    let mut i = pos;
                    for c in insert_chars {
                        chars.insert(i, c);
                        i += 1;
                    }
                }
            }
        }
        chars.into_iter().collect()
    }

    fn roundtrip(old: &str, new: &str) {
        let ops = compute_diff(old, new);
        let result = apply_ops(old, &ops);
        assert_eq!(
            result, new,
            "roundtrip failed: old={old:?} new={new:?} ops={ops:?}"
        );
    }

    #[test]
    fn empty_to_nonempty() {
        roundtrip("", "hello");
    }

    #[test]
    fn nonempty_to_empty() {
        roundtrip("hello", "");
    }

    #[test]
    fn identical_returns_no_ops() {
        assert!(compute_diff("hello", "hello").is_empty());
    }

    #[test]
    fn single_char_insert() {
        roundtrip("hllo", "hello");
    }

    #[test]
    fn single_char_delete() {
        roundtrip("hello", "hllo");
    }

    #[test]
    fn replace_word() {
        roundtrip("hello world", "hello beautiful world");
    }

    #[test]
    fn replace_at_start() {
        roundtrip("hello world", "hi world");
    }

    #[test]
    fn replace_at_end() {
        roundtrip("hello world", "hello earth");
    }

    #[test]
    fn full_replace() {
        roundtrip("hello", "world");
    }

    #[test]
    fn multiline() {
        roundtrip("line1\nline2\nline3", "line1\nmodified\nline3");
    }

    #[test]
    fn prepend_lines() {
        roundtrip("b\nc\n", "a\nb\nc\n");
    }

    #[test]
    fn append_lines() {
        roundtrip("a\nb\n", "a\nb\nc\n");
    }

    #[test]
    fn unicode_multibyte() {
        // CJK characters are a single Unicode scalar each
        roundtrip("你好", "你好世界");
        roundtrip("hello", "héllo");
    }

    #[test]
    fn emoji() {
        roundtrip("hi 🌍", "hi 🌍!");
        roundtrip("a🎉b", "a🎊b");
    }

    #[test]
    fn large_document_append() {
        let old: String = "a".repeat(10_000);
        let new = format!("{old}EXTRA");
        roundtrip(&old, &new);
    }

    #[test]
    fn large_document_prefix_insert() {
        let base: String = "z".repeat(5_000);
        let new = format!("PREFIX{base}");
        roundtrip(&base, &new);
    }
}
