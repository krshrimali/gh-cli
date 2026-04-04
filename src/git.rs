//! Local repository hints for opening PRs from the current checkout.

/// Current branch name, or `None` if detached / not a git repo.
pub fn current_branch() -> Option<String> {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() || s == "HEAD" {
        return None;
    }
    Some(s)
}

/// Short name of `origin`'s default branch (e.g. `main`), if configured.
pub fn default_base_branch() -> Option<String> {
    let out = std::process::Command::new("git")
        .args(["symbolic-ref", "-q", "refs/remotes/origin/HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let s = String::from_utf8_lossy(&out.stdout);
    s.trim()
        .strip_prefix("refs/remotes/origin/")
        .map(std::string::ToString::to_string)
}

/// `(head, base)` for the “new PR” wizard (`n` / `:create` / `:pr`).
pub fn pr_wizard_defaults() -> (String, String) {
    let head = current_branch().unwrap_or_default();
    let base = default_base_branch().unwrap_or_else(|| "main".to_string());
    (head, base)
}
