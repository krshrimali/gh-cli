mod app;
mod diff_pick;
mod diff_nvim;
mod editor;
mod git;
mod github;
mod markdown_render;
mod ui;

use anyhow::Context;
use app::App;
use clap::Parser;

/// Terminal UI for GitHub pull requests (comments, reviews, commits, merge, …) using octocrab.
#[derive(Parser, Debug)]
#[command(name = "gh-pr-cli")]
struct Cli {
    #[arg(long, help = "Repository owner (org or user)")]
    owner: Option<String>,
    #[arg(long, help = "Repository name")]
    repo: Option<String>,
    /// PR list status filter: open | closed | merged | draft | all (overrides `GH_PR_CLI_STATUS`).
    #[arg(long, value_name = "STATE")]
    status: Option<String>,
}

fn parse_github_remote(raw: &str) -> Option<(String, String)> {
    let s = raw.trim();
    let rest = s
        .strip_prefix("git@github.com:")
        .or_else(|| s.strip_prefix("ssh://git@github.com/"))?;
    let rest = rest.strip_suffix(".git").unwrap_or(rest);
    let mut parts = rest.splitn(2, '/');
    let o = parts.next()?.to_string();
    let r = parts.next()?.to_string();
    Some((o, r))
}

fn parse_https_remote(s: &str) -> Option<(String, String)> {
    let s = s.trim().strip_suffix(".git").unwrap_or(s.trim());
    let rest = s.strip_prefix("https://github.com/")?;
    let mut parts = rest.splitn(2, '/');
    let o = parts.next()?.to_string();
    let r = parts.next()?.to_string();
    Some((o, r))
}

fn repo_from_git() -> anyhow::Result<Option<(String, String)>> {
    let out = std::process::Command::new("git")
        .args(["remote", "get-url", "origin"])
        .output()
        .ok()
        .filter(|o| o.status.success());
    let Some(out) = out else {
        return Ok(None);
    };
    let url = String::from_utf8_lossy(&out.stdout);
    Ok(parse_github_remote(&url).or_else(|| parse_https_remote(&url)))
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let rt = tokio::runtime::Runtime::new().context("tokio runtime")?;

    // Build client only; `/user` runs after the first PR page so the TUI appears faster.
    let octo = rt.block_on(async { github::client_from_env() })?;

    let (owner, repo) = match (cli.owner, cli.repo) {
        (Some(o), Some(r)) => (o, r),
        (None, None) => repo_from_git()?.context(
            "pass --owner and --repo, or run inside a git repo whose origin is github.com",
        )?,
        _ => anyhow::bail!("provide both --owner and --repo (or neither to use git remote)"),
    };

    let status_cli = if let Some(s) = cli.status.as_deref() {
        let t = s.trim();
        Some(github::parse_pr_status_filter(t).ok_or_else(|| {
            anyhow::anyhow!(
                "invalid --status {t:?}: use open, closed, merged, draft, or all"
            )
        })?)
    } else {
        None
    };
    let mut app = App::new(owner, repo, octo, None, status_cli);
    let mut terminal = ratatui::try_init().context("terminal init")?;
    let result = app.run(&mut terminal, &rt);
    ratatui::restore();
    result
}
