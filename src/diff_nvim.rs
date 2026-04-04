//! Build a single markdown buffer: threaded review comments + full diff for Neovim.

use crate::app::ThreadItem;
use octocrab::models::CommentId;
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::PathBuf;

/// Writes `PR #n` review document and returns the path (kept on disk for editing).
pub fn write_pr_review_nvim_buffer(
    pr: u64,
    owner: &str,
    repo: &str,
    diff: &str,
    items: &[ThreadItem],
) -> anyhow::Result<PathBuf> {
    let mut f = tempfile::Builder::new()
        .prefix(&format!("gh-pr-{pr}-"))
        .suffix(".md")
        .tempfile()?;
    let body = build_markdown(pr, owner, repo, diff, items);
    f.write_all(body.as_bytes())?;
    let (_file, path) = f.keep()?;
    Ok(path)
}

fn build_markdown(pr: u64, owner: &str, repo: &str, diff: &str, items: &[ThreadItem]) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "# PR #{pr} ({owner}/{repo})\n\n\
         <!-- Neovim: set ft=markdown | threaded review export from gh-pr-cli -->\n\n"
    ));

    let mut review_by_id: HashMap<CommentId, &ThreadItem> = HashMap::new();
    let mut parent_of: HashMap<CommentId, CommentId> = HashMap::new();
    for it in items {
        if let ThreadItem::Review {
            id,
            in_reply_to,
            ..
        } = it
        {
            review_by_id.insert(*id, it);
            if let Some(p) = in_reply_to {
                parent_of.insert(*id, *p);
            }
        }
    }

    let mut is_root = HashSet::new();
    for it in items {
        if let ThreadItem::Review { id, .. } = it {
            if !parent_of.contains_key(id) {
                is_root.insert(*id);
            }
        }
    }

    out.push_str("## Conversation (issue comments)\n\n");
    for it in items {
        if let ThreadItem::Issue {
            author,
            body,
            created,
            ..
        } = it
        {
            out.push_str(&format!(
                "### @{author} — {created}\n\n{body}\n\n---\n\n"
            ));
        }
    }

    out.push_str("## Review threads (inline comments)\n\n");
    let mut roots: Vec<CommentId> = review_by_id
        .keys()
        .copied()
        .filter(|id| is_root.contains(id))
        .collect();
    roots.sort_by_key(|id| {
        review_by_id
            .get(id)
            .and_then(|t| match t {
                ThreadItem::Review { created, .. } => Some(*created),
                _ => None,
            })
            .unwrap_or_default()
    });

    for root in roots {
        emit_review_thread(&mut out, root, &review_by_id, &parent_of);
    }

    out.push_str("\n## Full diff\n\n``````diff\n");
    out.push_str(diff);
    if !diff.ends_with('\n') {
        out.push('\n');
    }
    out.push_str("``````\n");
    out
}

fn emit_review_thread(
    out: &mut String,
    root: CommentId,
    by_id: &HashMap<CommentId, &ThreadItem>,
    parent_of: &HashMap<CommentId, CommentId>,
) {
    let mut children_by_parent: HashMap<CommentId, Vec<CommentId>> = HashMap::new();
    for (&cid, &pid) in parent_of {
        children_by_parent.entry(pid).or_default().push(cid);
    }
    for v in children_by_parent.values_mut() {
        v.sort_by_key(|id| {
            by_id
                .get(id)
                .and_then(|t| match t {
                    ThreadItem::Review { created, .. } => Some(*created),
                    _ => None,
                })
                .unwrap_or_default()
        });
    }
    fn collect_depth_first(
        id: CommentId,
        children_by_parent: &HashMap<CommentId, Vec<CommentId>>,
        acc: &mut Vec<CommentId>,
    ) {
        acc.push(id);
        if let Some(ch) = children_by_parent.get(&id) {
            for c in ch {
                collect_depth_first(*c, children_by_parent, acc);
            }
        }
    }
    let mut chain: Vec<CommentId> = Vec::new();
    collect_depth_first(root, &children_by_parent, &mut chain);

    out.push_str("### Thread\n\n");
    for (i, cid) in chain.iter().enumerate() {
        let Some(ThreadItem::Review {
            author,
            body,
            path,
            line,
            diff_hunk,
            created,
            ..
        }) = by_id.get(cid).copied()
        else {
            continue;
        };
        let indent = "  ".repeat(i);
        out.push_str(&format!(
            "{indent}#### [{i}] @{author} — `{path}` L{line:?} — {created}\n\n"
        ));
        if !body.trim().is_empty() {
            out.push_str(&format!("{indent}{body}\n\n"));
        }
        let hunk: String = diff_hunk.chars().take(8000).collect();
        if !hunk.trim().is_empty() {
            out.push_str(&format!("{indent}``````diff\n{indent}{hunk}\n{indent}``````\n\n"));
        }
        out.push_str(&format!("{indent}---\n\n"));
    }
}
