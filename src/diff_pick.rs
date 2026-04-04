//! Slice a multi-file unified diff and map hunk lines to GitHub review-comment anchors.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffDisplayLine {
    pub text: String,
    /// `(line_number, "RIGHT" | "LEFT")` when this row can host a pull review comment.
    pub anchor: Option<(u32, &'static str)>,
}

/// True if this diff chunk is for `path` (as returned by the files API).
pub fn chunk_matches_path(chunk: &str, path: &str) -> bool {
    for ln in chunk.lines() {
        if let Some(p) = ln.strip_prefix("+++ b/") {
            if p.split('\t').next().map(|s| s.trim()) == Some(path) {
                return true;
            }
        }
        if let Some(p) = ln.strip_prefix("--- a/") {
            if p.split('\t').next().map(|s| s.trim()) == Some(path) {
                return true;
            }
        }
        if let Some(rest) = ln.strip_prefix("diff --git ") {
            let mut toks = rest.split_whitespace();
            let a = toks.next().unwrap_or("");
            let b = toks.next().unwrap_or("");
            let na = a.strip_prefix("a/").unwrap_or(a);
            let nb = b.strip_prefix("b/").unwrap_or(b);
            if na == path || nb == path {
                return true;
            }
        }
    }
    false
}

/// Returns one file's unified diff chunk (including the `diff --git` header), or `None`.
pub fn extract_file_patch<'a>(full_diff: &'a str, path: &str) -> Option<&'a str> {
    let marker = "diff --git ";
    let mut search_from = 0usize;
    while let Some(rel) = full_diff[search_from..].find(marker) {
        let start = search_from + rel;
        let after_marker = start + marker.len();
        let tail = &full_diff[after_marker..];
        let end_rel = tail
            .find("\ndiff --git ")
            .unwrap_or(tail.len());
        let chunk = &full_diff[start..after_marker + end_rel];
        if chunk_matches_path(chunk, path) {
            return Some(chunk);
        }
        search_from = after_marker;
    }
    None
}

fn parse_hunk_start(line: &str) -> Option<(u32, u32)> {
    let s = line.strip_prefix("@@")?.trim();
    let mut parts = s.split_whitespace();
    let old_tok = parts.next()?;
    let new_tok = parts.next()?;
    let old_tok = old_tok.strip_prefix('-')?;
    let new_tok = new_tok.strip_prefix('+')?;
    let old_start: u32 = old_tok.split(',').next()?.parse().ok()?;
    let new_start: u32 = new_tok.split(',').next()?.parse().ok()?;
    Some((old_start, new_start))
}

/// Build scrollable rows with optional GitHub `(line, side)` anchors.
pub fn parse_patch_lines(patch: &str) -> Vec<DiffDisplayLine> {
    let mut out = Vec::new();
    let mut in_hunk = false;
    let mut old_cur = 0u32;
    let mut new_cur = 0u32;

    for raw in patch.lines() {
        let line = raw.trim_end_matches('\r');
        if line.starts_with("@@ ") {
            if let Some((os, ns)) = parse_hunk_start(line) {
                old_cur = os;
                new_cur = ns;
                in_hunk = true;
            } else {
                in_hunk = false;
            }
            out.push(DiffDisplayLine {
                text: line.to_string(),
                anchor: None,
            });
            continue;
        }

        if !in_hunk || line.is_empty() {
            out.push(DiffDisplayLine {
                text: line.to_string(),
                anchor: None,
            });
            continue;
        }

        let first = line.chars().next().unwrap_or(' ');

        match first {
            ' ' => {
                let label = format!("{new_cur:>4}→ {line}");
                out.push(DiffDisplayLine {
                    text: label,
                    anchor: Some((new_cur, "RIGHT")),
                });
                old_cur = old_cur.saturating_add(1);
                new_cur = new_cur.saturating_add(1);
            }
            '-' => {
                let label = format!("{old_cur:>4}← {line}");
                out.push(DiffDisplayLine {
                    text: label,
                    anchor: Some((old_cur, "LEFT")),
                });
                old_cur = old_cur.saturating_add(1);
            }
            '+' => {
                let label = format!("{new_cur:>4}→ {line}");
                out.push(DiffDisplayLine {
                    text: label,
                    anchor: Some((new_cur, "RIGHT")),
                });
                new_cur = new_cur.saturating_add(1);
            }
            '\\' => {
                out.push(DiffDisplayLine {
                    text: line.to_string(),
                    anchor: None,
                });
            }
            _ => {
                out.push(DiffDisplayLine {
                    text: line.to_string(),
                    anchor: None,
                });
            }
        }
    }

    out
}

pub fn first_anchor_index(lines: &[DiffDisplayLine]) -> Option<usize> {
    lines.iter().position(|l| l.anchor.is_some())
}

/// Next/previous index that has a GitHub anchor (skips headers and `@@` lines).
pub fn step_anchor(cursor: usize, lines: &[DiffDisplayLine], forward: bool) -> usize {
    if lines.is_empty() {
        return 0;
    }
    let n = lines.len();
    for step in 1..=n {
        let j = if forward {
            (cursor + step) % n
        } else {
            (cursor + n - (step % n)) % n
        };
        if lines[j].anchor.is_some() {
            return j;
        }
    }
    cursor.min(n.saturating_sub(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_and_parse() {
        let full = "\
diff --git a/x b/x
--- a/x
+++ b/x
@@ -1,2 +1,2 @@
 a
-b
+c
";
        let chunk = extract_file_patch(full, "x").expect("chunk");
        let rows = parse_patch_lines(chunk);
        let anchors: Vec<_> = rows.iter().filter_map(|r| r.anchor).collect();
        assert!(anchors.contains(&(1, "RIGHT")));
        assert!(anchors.contains(&(2, "LEFT")));
        assert!(anchors.contains(&(2, "RIGHT")));
    }
}
