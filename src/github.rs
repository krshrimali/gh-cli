//! GitHub API via [octocrab](https://github.com/XAMPPRocky/octocrab).
//! Pull-review comment reactions use the REST routes directly where the typed API
//! only covers issue comments.

use anyhow::Context;
use octocrab::models::issues::{Comment as IssueComment, Issue};
use octocrab::models::pulls::{Comment as PullComment, PullRequest, Review, ReviewComment};
use octocrab::models::reactions::{Reaction, ReactionContent};
use octocrab::models::repos::{DiffEntry, RepoCommit};
use octocrab::models::CommentId;
use octocrab::params::{self, pulls::MergeMethod};

/// PR list status: maps to REST `state` when possible, otherwise to issue search qualifiers.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum PrStatusFilter {
    #[default]
    Open,
    Closed,
    Merged,
    Draft,
    All,
}

impl PrStatusFilter {
    pub fn label(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Closed => "closed",
            Self::Merged => "merged",
            Self::Draft => "draft",
            Self::All => "all",
        }
    }

    /// `GET /pulls` only supports open / closed / all.
    pub fn rest_state(self) -> Option<params::State> {
        match self {
            Self::Open => Some(params::State::Open),
            Self::Closed => Some(params::State::Closed),
            Self::All => Some(params::State::All),
            Self::Merged | Self::Draft => None,
        }
    }
}

pub fn parse_pr_status_filter(s: &str) -> Option<PrStatusFilter> {
    Some(match s.to_ascii_lowercase().as_str() {
        "open" => PrStatusFilter::Open,
        "closed" => PrStatusFilter::Closed,
        "merged" => PrStatusFilter::Merged,
        "draft" => PrStatusFilter::Draft,
        "all" => PrStatusFilter::All,
        _ => return None,
    })
}

use octocrab::{Octocrab, Page};
use serde::Serialize;

/// Page size for PR list requests (GitHub allows up to 100).
pub const PR_LIST_PER_PAGE: u8 = 30;

/// Optional filters for the PR list. Branch filters use the REST `/pulls` API when alone; author,
/// assignee, reviewers, mentions, and labels use the [search
/// API](https://docs.github.com/en/search-github/searching-on-github/searching-issues-and-pull-requests)
/// (`is:pr repo:…`).
#[derive(Clone, Default, Debug, PartialEq, Eq)]
pub struct PrListFilters {
    pub head: Option<String>,
    pub base: Option<String>,
    pub author: Option<String>,
    pub assignee: Option<String>,
    pub mentions: Option<String>,
    /// `review-requested:login` in search.
    pub review_requested: Option<String>,
    /// `reviewed-by:login` in search.
    pub reviewed_by: Option<String>,
    pub label: Option<String>,
    /// Substring search in PR titles (`in:title …` in issue search).
    pub title_search: Option<String>,
}

impl PrListFilters {
    pub fn any_field_set(&self) -> bool {
        filter_nonempty(&self.head)
            || filter_nonempty(&self.base)
            || filter_nonempty(&self.author)
            || filter_nonempty(&self.assignee)
            || filter_nonempty(&self.mentions)
            || filter_nonempty(&self.review_requested)
            || filter_nonempty(&self.reviewed_by)
            || filter_nonempty(&self.label)
            || filter_nonempty(&self.title_search)
    }
}

fn filter_nonempty(opt: &Option<String>) -> bool {
    opt.as_ref().map(|s| !s.trim().is_empty()).unwrap_or(false)
}

/// When true, list PRs via `GET /search/issues` (issue search with `is:pr`).
pub fn pr_list_uses_search(filters: &PrListFilters, status: PrStatusFilter) -> bool {
    filter_nonempty(&filters.author)
        || filter_nonempty(&filters.assignee)
        || filter_nonempty(&filters.mentions)
        || filter_nonempty(&filters.review_requested)
        || filter_nonempty(&filters.reviewed_by)
        || filter_nonempty(&filters.label)
        || filter_nonempty(&filters.title_search)
        || matches!(status, PrStatusFilter::Merged | PrStatusFilter::Draft)
}

fn search_token(value: &str) -> String {
    let t = value.trim();
    if t.is_empty() {
        return String::new();
    }
    if t.chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == '@' || c == '.')
    {
        t.to_string()
    } else {
        format!("\"{}\"", t.replace('\\', "\\\\").replace('"', "\\\""))
    }
}

/// Builds the `q` string for `search/issues`.
pub fn build_pr_search_query(
    owner: &str,
    repo: &str,
    status: PrStatusFilter,
    filters: &PrListFilters,
) -> String {
    let mut parts = vec![format!("repo:{}/{}", owner, repo), "is:pr".to_string()];
    match status {
        PrStatusFilter::Open => parts.push("is:open".into()),
        PrStatusFilter::Closed => parts.push("is:closed".into()),
        PrStatusFilter::Merged => {
            parts.push("is:merged".into());
        }
        PrStatusFilter::Draft => {
            parts.push("is:open".into());
            parts.push("draft:true".into());
        }
        PrStatusFilter::All => {}
    }
    if let Some(ref h) = filters.head {
        let t = h.trim();
        if !t.is_empty() {
            parts.push(format!("head:{}", search_token(t)));
        }
    }
    if let Some(ref b) = filters.base {
        let t = b.trim();
        if !t.is_empty() {
            parts.push(format!("base:{}", search_token(t)));
        }
    }
    if let Some(ref a) = filters.author {
        let t = a.trim();
        if !t.is_empty() {
            parts.push(format!("author:{}", search_token(t)));
        }
    }
    if let Some(ref a) = filters.assignee {
        let t = a.trim();
        if t.eq_ignore_ascii_case("none")
            || t == "-"
            || t.eq_ignore_ascii_case("no")
            || t.eq_ignore_ascii_case("unassigned")
        {
            parts.push("no:assignee".into());
        } else if !t.is_empty() {
            parts.push(format!("assignee:{}", search_token(t)));
        }
    }
    if let Some(ref m) = filters.mentions {
        let t = m.trim();
        if !t.is_empty() {
            parts.push(format!("mentions:{}", search_token(t)));
        }
    }
    if let Some(ref r) = filters.review_requested {
        let t = r.trim();
        if !t.is_empty() {
            parts.push(format!("review-requested:{}", search_token(t)));
        }
    }
    if let Some(ref r) = filters.reviewed_by {
        let t = r.trim();
        if !t.is_empty() {
            parts.push(format!("reviewed-by:{}", search_token(t)));
        }
    }
    if let Some(ref l) = filters.label {
        let t = l.trim();
        if !t.is_empty() {
            parts.push(format!("label:{}", search_token(t)));
        }
    }
    if let Some(ref ts) = filters.title_search {
        let t = ts.trim();
        if !t.is_empty() {
            parts.push(format!("in:title {}", search_token(t)));
        }
    }
    parts.join(" ")
}

pub enum PrListPage {
    Pulls(Page<PullRequest>),
    Issues(Page<Issue>),
}

pub async fn fetch_pr_list(
    oct: &Octocrab,
    owner: &str,
    repo: &str,
    status: PrStatusFilter,
    page_no: u32,
    per_page: u8,
    filters: &PrListFilters,
) -> anyhow::Result<PrListPage> {
    if pr_list_uses_search(filters, status) {
        let q = build_pr_search_query(owner, repo, status, filters);
        let page = oct
            .search()
            .issues_and_pull_requests(&q)
            .sort("updated")
            .order("desc")
            .per_page(per_page)
            .page(page_no)
            .send()
            .await?;
        Ok(PrListPage::Issues(page))
    } else {
        let state = status.rest_state().expect("REST path only for open/closed/all");
        let pulls = oct.pulls(owner, repo);
        let mut b = pulls.list().state(state).per_page(per_page).page(page_no);
        if let Some(ref h) = filters.head {
            let t = h.trim();
            if !t.is_empty() {
                b = b.head(t.to_string());
            }
        }
        if let Some(ref bs) = filters.base {
            let t = bs.trim();
            if !t.is_empty() {
                b = b.base(t.to_string());
            }
        }
        Ok(PrListPage::Pulls(b.send().await?))
    }
}

/// Resolves a GitHub API token. GitHub’s REST API is HTTPS + token-based; SSH keys (`~/.ssh/id_rsa`)
/// are only for the Git/SSH protocol, not for `api.github.com`, so octocrab cannot authenticate with
/// a raw private key. The practical options are a PAT or the token `gh auth login` stores.
pub fn resolve_github_token() -> anyhow::Result<String> {
    if let Ok(t) = std::env::var("GITHUB_TOKEN") {
        let t = t.trim();
        if !t.is_empty() {
            return Ok(t.to_string());
        }
    }

    let output = std::process::Command::new("gh")
        .args(["auth", "token"])
        .output()
        .map_err(|e| {
            anyhow::anyhow!(
                "could not run `gh auth token`: {e}\n\
                 Install GitHub CLI and run `gh auth login`, or set GITHUB_TOKEN.\n\
                 Note: SSH keys (id_rsa) are not used for the GitHub HTTP API."
            )
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "`gh auth token` failed (not logged in?).\n\
             Run:  gh auth login\n\
             Or set:  GITHUB_TOKEN\n\
             ({stderr})\n\
             SSH keys alone cannot authenticate api.github.com — `gh` still uses an OAuth token after login."
        );
    }

    let token = String::from_utf8(output.stdout)
        .context("gh auth token: invalid UTF-8")?
        .trim()
        .to_string();

    anyhow::ensure!(
        !token.is_empty(),
        "`gh auth token` returned nothing; try `gh auth login`"
    );

    Ok(token)
}

pub fn client_from_env() -> anyhow::Result<Octocrab> {
    let token = resolve_github_token()?;
    Octocrab::builder()
        .personal_token(token)
        .build()
        .context("failed to build octocrab client")
}

pub async fn current_login(oct: &Octocrab) -> anyhow::Result<Option<String>> {
    match oct.current().user().await {
        Ok(a) => Ok(Some(a.login)),
        Err(_) => Ok(None),
    }
}

pub async fn get_pull(
    oct: &Octocrab,
    owner: &str,
    repo: &str,
    number: u64,
) -> anyhow::Result<PullRequest> {
    Ok(oct.pulls(owner, repo).get(number).await?)
}

pub async fn list_issue_comments(
    oct: &Octocrab,
    owner: &str,
    repo: &str,
    issue_number: u64,
) -> anyhow::Result<Vec<IssueComment>> {
    let page = oct
        .issues(owner, repo)
        .list_comments(issue_number)
        .per_page(100)
        .send()
        .await?;
    Ok(oct.all_pages(page).await?)
}

pub async fn list_review_comments(
    oct: &Octocrab,
    owner: &str,
    repo: &str,
    pr: u64,
) -> anyhow::Result<Vec<PullComment>> {
    let page = oct
        .pulls(owner, repo)
        .list_comments(Some(pr))
        .per_page(100)
        .send()
        .await?;
    Ok(oct.all_pages(page).await?)
}

pub async fn list_pr_commits(
    oct: &Octocrab,
    owner: &str,
    repo: &str,
    pr: u64,
) -> anyhow::Result<Vec<RepoCommit>> {
    let page = oct.pulls(owner, repo).pr_commits(pr).per_page(100).send().await?;
    Ok(oct.all_pages(page).await?)
}

pub async fn list_pr_files(
    oct: &Octocrab,
    owner: &str,
    repo: &str,
    pr: u64,
) -> anyhow::Result<Vec<DiffEntry>> {
    let page = oct.pulls(owner, repo).list_files(pr).await?;
    Ok(oct.all_pages(page).await?)
}

pub async fn get_pr_diff(oct: &Octocrab, owner: &str, repo: &str, pr: u64) -> anyhow::Result<String> {
    Ok(oct.pulls(owner, repo).get_diff(pr).await?)
}

pub async fn list_reviews(
    oct: &Octocrab,
    owner: &str,
    repo: &str,
    pr: u64,
) -> anyhow::Result<Vec<Review>> {
    let page = oct
        .pulls(owner, repo)
        .list_reviews(pr)
        .per_page(100)
        .send()
        .await?;
    Ok(oct.all_pages(page).await?)
}

pub async fn create_issue_comment(
    oct: &Octocrab,
    owner: &str,
    repo: &str,
    issue_number: u64,
    body: &str,
) -> anyhow::Result<IssueComment> {
    Ok(oct
        .issues(owner, repo)
        .create_comment(issue_number, body)
        .await?)
}

pub async fn update_issue_comment(
    oct: &Octocrab,
    owner: &str,
    repo: &str,
    id: CommentId,
    body: &str,
) -> anyhow::Result<IssueComment> {
    Ok(oct
        .issues(owner, repo)
        .update_comment(id, body)
        .await?)
}

pub async fn delete_issue_comment(
    oct: &Octocrab,
    owner: &str,
    repo: &str,
    id: CommentId,
) -> anyhow::Result<()> {
    Ok(oct.issues(owner, repo).delete_comment(id).await?)
}

pub async fn update_review_comment(
    oct: &Octocrab,
    owner: &str,
    repo: &str,
    id: CommentId,
    body: &str,
) -> anyhow::Result<PullComment> {
    Ok(oct.pulls(owner, repo).comment(id).update(body).await?)
}

pub async fn delete_review_comment(
    oct: &Octocrab,
    owner: &str,
    repo: &str,
    id: CommentId,
) -> anyhow::Result<()> {
    Ok(oct.pulls(owner, repo).comment(id).delete().await?)
}

pub async fn reply_to_review_comment(
    oct: &Octocrab,
    owner: &str,
    repo: &str,
    pr: u64,
    in_reply_to: CommentId,
    body: &str,
) -> anyhow::Result<ReviewComment> {
    Ok(oct
        .pulls(owner, repo)
        .reply_to_comment(pr, in_reply_to, body)
        .await?)
}

#[derive(Serialize)]
struct ReactionBody {
    content: ReactionContent,
}

pub async fn create_issue_comment_reaction(
    oct: &Octocrab,
    owner: &str,
    repo: &str,
    comment_id: CommentId,
    content: ReactionContent,
) -> anyhow::Result<Reaction> {
    Ok(oct
        .issues(owner, repo)
        .create_comment_reaction(comment_id, content)
        .await?)
}

pub async fn create_pull_comment_reaction(
    oct: &Octocrab,
    owner: &str,
    repo: &str,
    comment_id: CommentId,
    content: ReactionContent,
) -> anyhow::Result<Reaction> {
    let route = format!(
        "/repos/{owner}/{repo}/pulls/comments/{comment_id}/reactions",
        comment_id = comment_id
    );
    Ok(oct
        .post(route.as_str(), Some(&ReactionBody { content }))
        .await?)
}

pub async fn list_issue_comment_reactions(
    oct: &Octocrab,
    owner: &str,
    repo: &str,
    comment_id: CommentId,
) -> anyhow::Result<Vec<Reaction>> {
    let route = format!(
        "/repos/{owner}/{repo}/issues/comments/{comment_id}/reactions",
        comment_id = comment_id
    );
    Ok(oct.get(route.as_str(), None::<&()>).await?)
}

pub async fn list_pull_comment_reactions(
    oct: &Octocrab,
    owner: &str,
    repo: &str,
    comment_id: CommentId,
) -> anyhow::Result<Vec<Reaction>> {
    let route = format!(
        "/repos/{owner}/{repo}/pulls/comments/{comment_id}/reactions",
        comment_id = comment_id
    );
    Ok(oct.get(route, None::<&()>).await?)
}

pub async fn create_pull(
    oct: &Octocrab,
    owner: &str,
    repo: &str,
    title: &str,
    head: &str,
    base: &str,
    body: Option<&str>,
    draft: bool,
) -> anyhow::Result<PullRequest> {
    let pulls = oct.pulls(owner, repo);
    let mut b = pulls.create(title, head, base);
    if let Some(bd) = body {
        b = b.body(bd.to_string());
    }
    b = b.draft(Some(draft));
    Ok(b.send().await?)
}

pub async fn update_pull_body(
    oct: &Octocrab,
    owner: &str,
    repo: &str,
    number: u64,
    body: &str,
) -> anyhow::Result<PullRequest> {
    Ok(oct
        .pulls(owner, repo)
        .update(number)
        .body(body)
        .send()
        .await?)
}

pub async fn merge_pull(
    oct: &Octocrab,
    owner: &str,
    repo: &str,
    number: u64,
    method: MergeMethod,
) -> anyhow::Result<()> {
    let _ = oct
        .pulls(owner, repo)
        .merge(number)
        .method(method)
        .send()
        .await?;
    Ok(())
}

pub async fn update_pr_branch(oct: &Octocrab, owner: &str, repo: &str, number: u64) -> anyhow::Result<bool> {
    Ok(oct.pulls(owner, repo).update_branch(number).await?)
}
