use crate::diff_nvim;
use crate::diff_pick;
use crate::editor;
use crate::git;
use crate::markdown_render;
use crate::ui;
use crate::github;
use std::collections::HashMap;
use anyhow::Context;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::widgets::ListState;
use std::cell::Cell;
use octocrab::models::issues::Issue;
use octocrab::models::pulls::PullRequest;
use octocrab::models::repos::RepoCommit;
use octocrab::models::CommentId;
use octocrab::models::reactions::ReactionContent;
use octocrab::params::pulls::MergeMethod;
use octocrab::Octocrab;
use ratatui::DefaultTerminal;
use tokio::runtime::Runtime;

#[derive(Clone)]
pub enum ThreadItem {
    Issue {
        id: CommentId,
        author: String,
        body: String,
        created: chrono::DateTime<chrono::Utc>,
    },
    Review {
        id: CommentId,
        author: String,
        body: String,
        path: String,
        line: Option<u64>,
        diff_hunk: String,
        in_reply_to: Option<CommentId>,
        created: chrono::DateTime<chrono::Utc>,
    },
}

impl ThreadItem {
    pub(crate) fn created(&self) -> chrono::DateTime<chrono::Utc> {
        match self {
            ThreadItem::Issue { created, .. } => *created,
            ThreadItem::Review { created, .. } => *created,
        }
    }

    pub(crate) fn id(&self) -> CommentId {
        match self {
            ThreadItem::Issue { id, .. } => *id,
            ThreadItem::Review { id, .. } => *id,
        }
    }

    pub(crate) fn author(&self) -> &str {
        match self {
            ThreadItem::Issue { author, .. } => author,
            ThreadItem::Review { author, .. } => author,
        }
    }

    pub(crate) fn body(&self) -> &str {
        match self {
            ThreadItem::Issue { body, .. } => body,
            ThreadItem::Review { body, .. } => body,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PrTab {
    Info,
    Thread,
    Commits,
    Files,
    Diff,
    Reviews,
}

impl PrTab {
    fn from_digit(d: u8) -> Option<Self> {
        Some(match d {
            1 => Self::Info,
            2 => Self::Thread,
            3 => Self::Commits,
            4 => Self::Files,
            5 => Self::Diff,
            6 => Self::Reviews,
            _ => return None,
        })
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Info => "1 Info",
            Self::Thread => "2 Thread",
            Self::Commits => "3 Commits",
            Self::Files => "4 Files",
            Self::Diff => "5 Diff",
            Self::Reviews => "6 Reviews",
        }
    }
}

/// One row in the PR list (REST `PullRequest` or search `Issue` with `pull_request` set).
#[derive(Clone)]
pub enum PrListEntry {
    Rest(PullRequest),
    Search(Issue),
}

impl PrListEntry {
    pub fn number(&self) -> u64 {
        match self {
            Self::Rest(p) => p.number,
            Self::Search(i) => i.number,
        }
    }

    pub fn title(&self) -> &str {
        match self {
            Self::Rest(p) => p.title.as_deref().unwrap_or("(no title)"),
            Self::Search(i) => i.title.as_str(),
        }
    }

    pub fn author_login(&self) -> &str {
        match self {
            Self::Rest(p) => p.user.as_ref().map(|u| u.login.as_str()).unwrap_or("?"),
            Self::Search(i) => i.user.login.as_str(),
        }
    }

    pub fn state_display(&self) -> String {
        match self {
            Self::Rest(p) => p
                .state
                .as_ref()
                .map(|s| format!("{s:?}"))
                .unwrap_or_else(|| "?".into()),
            Self::Search(i) => format!("{:?}", i.state),
        }
    }

    pub fn html_url_open(&self) -> Option<String> {
        match self {
            Self::Rest(p) => p.html_url.as_ref().map(|u| u.to_string()),
            Self::Search(i) => Some(i.html_url.to_string()),
        }
    }

    /// Short badges: merged / draft / state.
    pub fn status_badges(&self) -> String {
        match self {
            Self::Rest(p) => {
                let mut s = String::new();
                if p.draft == Some(true) {
                    s.push_str("D·");
                }
                if p.merged == Some(true) {
                    s.push_str("M·");
                }
                s.push_str(
                    &p.state
                        .as_ref()
                        .map(|st| format!("{st:?}"))
                        .unwrap_or_else(|| "?".into()),
                );
                s
            }
            Self::Search(i) => format!("{:?}", i.state),
        }
    }

    pub(crate) fn ellipsize(s: &str, max_chars: usize) -> String {
        let t = s.trim();
        if t.chars().count() <= max_chars {
            return t.to_string();
        }
        format!(
            "{}…",
            t.chars().take(max_chars.saturating_sub(1)).collect::<String>()
        )
    }

    /// Second line of the PR list: assignees, reviewers (REST only), labels.
    pub fn meta_summary(&self) -> String {
        match self {
            Self::Rest(p) => {
                let asg = p
                    .assignees
                    .as_ref()
                    .map(|v| {
                        v.iter()
                            .map(|a| a.login.as_str())
                            .take(4)
                            .collect::<Vec<_>>()
                            .join(",")
                    })
                    .filter(|s| !s.is_empty())
                    .map(|s| format!("as:{s}"))
                    .unwrap_or_default();
                let rv = p
                    .requested_reviewers
                    .as_ref()
                    .map(|v| {
                        v.iter()
                            .map(|a| a.login.as_str())
                            .take(4)
                            .collect::<Vec<_>>()
                            .join(",")
                    })
                    .filter(|s| !s.is_empty())
                    .map(|s| format!("rv:{s}"))
                    .unwrap_or_default();
                let lb = p
                    .labels
                    .as_ref()
                    .map(|v| {
                        v.iter()
                            .map(|l| l.name.as_str())
                            .take(5)
                            .collect::<Vec<_>>()
                            .join(",")
                    })
                    .filter(|s| !s.is_empty())
                    .map(|s| format!("lb:{s}"))
                    .unwrap_or_default();
                let parts = [asg, rv, lb]
                    .into_iter()
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join("  ");
                Self::ellipsize(&parts, 100)
            }
            Self::Search(i) => {
                let asg = if i.assignees.is_empty() {
                    String::new()
                } else {
                    format!(
                        "as:{}",
                        i.assignees
                            .iter()
                            .map(|a| a.login.as_str())
                            .take(4)
                            .collect::<Vec<_>>()
                            .join(",")
                    )
                };
                let lb = if i.labels.is_empty() {
                    String::new()
                } else {
                    format!(
                        "lb:{}",
                        i.labels
                            .iter()
                            .map(|l| l.name.as_str())
                            .take(5)
                            .collect::<Vec<_>>()
                            .join(",")
                    )
                };
                let parts = [asg, lb]
                    .into_iter()
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join("  ");
                Self::ellipsize(&parts, 100)
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    PrList,
    PrDetail,
}

/// `f` filter panel: overview text, or status picker (`s` / `A`).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FilterPanelPhase {
    Overview,
    StatusPick { cursor: usize },
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum InlineReviewPhase {
    PickFile,
    PickLine,
}

pub(crate) fn pr_status_menu_cursor(st: github::PrStatusFilter) -> usize {
    match st {
        github::PrStatusFilter::Open => 0,
        github::PrStatusFilter::Closed => 1,
        github::PrStatusFilter::Merged => 2,
        github::PrStatusFilter::Draft => 3,
        github::PrStatusFilter::All => 4,
    }
}

#[derive(Clone)]
pub enum Overlay {
    None,
    Help,
    Command,
    ReactionPicker,
    CreatePrWizard {
        phase: u8,
        title: String,
        head: String,
        base: String,
        buf: String,
    },
    /// PR filters + status (`f`; `s` opens status, `A` jumps to status).
    FilterSummary(FilterPanelPhase),
    /// Reviews tab: pick file → diff line → `$EDITOR` → POST review comment.
    InlineReview {
        phase: InlineReviewPhase,
        file_cursor: usize,
        path: String,
        diff_lines: Vec<diff_pick::DiffDisplayLine>,
        line_cursor: usize,
    },
}

pub enum EditorIntent {
    NewComment { pr: u64 },
    Reply { pr: u64, to: CommentId },
    EditIssue { id: CommentId },
    EditReview { id: CommentId },
    CreatePull {
        title: String,
        head: String,
        base: String,
    },
    InlineReviewComment {
        pr: u64,
        commit_sha: String,
        path: String,
        line: u32,
        side: String,
    },
}

pub enum AppEffect {
    None,
    Quit,
    OpenEditor {
        initial: String,
        intent: EditorIntent,
    },
    OpenNvim {
        path: std::path::PathBuf,
    },
    /// Try `kitten icat` (Kitty) for a remote image URL after suspending the TUI.
    KittyIcat {
        url: String,
    },
    /// Open buffer in `$VISUAL` / `$EDITOR` (view-only; save optional).
    ViewInEditor {
        text: String,
        ext: &'static str,
    },
}

pub struct App {
    pub owner: String,
    pub repo: String,
    pub octo: Octocrab,
    pub me: Option<String>,
    pub screen: Screen,
    pub pr_number: Option<u64>,
    pub status: String,
    pub pr_status: github::PrStatusFilter,
    pub pr_filters: github::PrListFilters,
    pub pr_entries: Vec<PrListEntry>,
    /// From search API `total_count` when listing via search; otherwise None.
    pub pr_list_total_count: Option<u64>,
    /// Last fetched GitHub page index (1-based). Reset when refreshing.
    pub pr_list_page: u32,
    /// More pages available (`Link: rel="next"`).
    pub pr_list_has_more: bool,
    pub pr_cursor: usize,
    pub pr_scroll: usize,
    pub current_pr: Option<PullRequest>,
    pub pr_tab: PrTab,
    pub tab_scroll: usize,
    pub thread_items: Vec<ThreadItem>,
    pub thread_cursor: usize,
    pub thread_scroll: usize,
    pub thread_detail_scroll: usize,
    pub reactions_line: Option<String>,
    /// Reactions loaded with `L` (not refetched on every j/k).
    reactions_cache: HashMap<CommentId, String>,
    /// First `![](url)` in the selected thread comment (for `I` / Kitty).
    pub thread_image_url: Option<String>,
    /// Scroll for inline review diff hunk pane.
    pub thread_hunk_scroll: usize,
    pub commits: Vec<RepoCommit>,
    pub commit_cursor: usize,
    pub commit_scroll: usize,
    pub files_lines: Vec<String>,
    /// Parallel to `files_lines`: raw paths from the API (for inline review + matching `+++ b/`).
    pub file_paths: Vec<String>,
    pub file_cursor: usize,
    pub file_scroll: usize,
    pub diff_text: String,
    pub diff_scroll: usize,
    pub reviews_lines: Vec<String>,
    pub review_cursor: usize,
    pub review_scroll: usize,
    /// Stateful list scroll for Thread and Reviews panes.
    pub thread_list_state: ListState,
    pub review_list_state: ListState,
    pub inline_review_file_state: ListState,
    pub inline_review_line_state: ListState,
    /// Inner rect of the inline-review list (mouse row → index).
    pub wizard_hit_rect: Cell<Option<Rect>>,
    pub overlay: Overlay,
    pub command_buf: String,
    pub vim_g_pending: bool,
    pub reaction_cursor: usize,
    pub loading: bool,
    /// Inner rect of the PR list widget (for mouse hit-testing). Reset each frame.
    pub pr_list_hit_rect: Cell<Option<Rect>>,
}

impl App {
    /// `status_cli` is applied after env defaults and wins over `GH_PR_CLI_STATUS`.
    pub fn new(
        owner: String,
        repo: String,
        octo: Octocrab,
        me: Option<String>,
        status_cli: Option<github::PrStatusFilter>,
    ) -> Self {
        let mut s = Self {
            owner,
            repo,
            octo,
            me,
            screen: Screen::PrList,
            pr_number: None,
            status: "j/k  click row  A status▼  f filters  m more  Enter open  a cycle state  : cmd  ? help  q"
                .to_string(),
            pr_status: github::PrStatusFilter::Open,
            pr_filters: github::PrListFilters::default(),
            pr_entries: Vec::new(),
            pr_list_total_count: None,
            pr_list_page: 0,
            pr_list_has_more: false,
            pr_cursor: 0,
            pr_scroll: 0,
            current_pr: None,
            pr_tab: PrTab::Thread,
            tab_scroll: 0,
            thread_items: Vec::new(),
            thread_cursor: 0,
            thread_scroll: 0,
            thread_detail_scroll: 0,
            reactions_line: None,
            reactions_cache: HashMap::new(),
            thread_image_url: None,
            thread_hunk_scroll: 0,
            commits: Vec::new(),
            commit_cursor: 0,
            commit_scroll: 0,
            files_lines: Vec::new(),
            file_paths: Vec::new(),
            file_cursor: 0,
            file_scroll: 0,
            diff_text: String::new(),
            diff_scroll: 0,
            reviews_lines: Vec::new(),
            review_cursor: 0,
            review_scroll: 0,
            thread_list_state: ListState::default(),
            review_list_state: ListState::default(),
            inline_review_file_state: ListState::default(),
            inline_review_line_state: ListState::default(),
            wizard_hit_rect: Cell::new(None),
            overlay: Overlay::None,
            command_buf: String::new(),
            vim_g_pending: false,
            reaction_cursor: 0,
            loading: false,
            pr_list_hit_rect: Cell::new(None),
        };
        s.apply_env_default_filters();
        if let Some(st) = status_cli {
            s.pr_status = st;
        }
        s
    }

    /// `GH_PR_CLI_STATUS`, `GH_PR_CLI_TITLE`, `GH_PR_CLI_AUTHOR`, `GH_PR_CLI_LABEL`, etc.
    fn apply_env_default_filters(&mut self) {
        if let Ok(v) = std::env::var("GH_PR_CLI_STATUS") {
            if let Some(st) = github::parse_pr_status_filter(v.trim()) {
                self.pr_status = st;
            }
        }
        if let Ok(v) = std::env::var("GH_PR_CLI_TITLE") {
            let t = v.trim();
            if !t.is_empty() {
                self.pr_filters.title_search = Some(t.to_string());
            }
        }
        if let Ok(v) = std::env::var("GH_PR_CLI_AUTHOR") {
            let t = v.trim();
            if !t.is_empty() {
                self.pr_filters.author = Some(t.to_string());
            }
        }
        if let Ok(v) = std::env::var("GH_PR_CLI_ASSIGNEE") {
            let t = v.trim();
            if !t.is_empty() {
                self.pr_filters.assignee = Some(t.to_string());
            }
        }
        if let Ok(v) = std::env::var("GH_PR_CLI_LABEL") {
            let t = v.trim();
            if !t.is_empty() {
                self.pr_filters.label = Some(t.to_string());
            }
        }
        if let Ok(v) = std::env::var("GH_PR_CLI_HEAD") {
            let t = v.trim();
            if !t.is_empty() {
                self.pr_filters.head = Some(t.to_string());
            }
        }
        if let Ok(v) = std::env::var("GH_PR_CLI_BASE") {
            let t = v.trim();
            if !t.is_empty() {
                self.pr_filters.base = Some(t.to_string());
            }
        }
        if let Ok(v) = std::env::var("GH_PR_CLI_REVIEWER") {
            let t = v.trim();
            if !t.is_empty() {
                self.pr_filters.review_requested = Some(t.to_string());
            }
        }
    }

    pub fn run(&mut self, terminal: &mut DefaultTerminal, rt: &Runtime) -> anyhow::Result<()> {
        let _mouse_guard = MouseCaptureGuard::new()?;

        self.set_status("Loading first page of pull requests…");
        self.loading = true;
        terminal.draw(|f| ui::draw(f, self))?;
        self.refresh_pr_list(rt)?;
        loop {
            terminal.draw(|f| ui::draw(f, self))?;
            if !crossterm::event::poll(std::time::Duration::from_millis(250))? {
                continue;
            }
            match crossterm::event::read()? {
                Event::Key(key) => match self.handle_key(key, rt)? {
                    AppEffect::Quit => break,
                    AppEffect::OpenEditor { initial, intent } => {
                        ratatui::restore();
                        let out = editor::edit_string(&initial);
                        *terminal = ratatui::try_init().context("terminal re-init")?;
                        self.apply_editor_result(intent, out, rt)?;
                    }
                    AppEffect::OpenNvim { path } => {
                        ratatui::restore();
                        let bin =
                            std::env::var("GH_PR_CLI_NVIM").unwrap_or_else(|_| "nvim".into());
                        let st = std::process::Command::new(&bin).arg(&path).status();
                        *terminal = ratatui::try_init().context("terminal re-init after nvim")?;
                        match st {
                            Ok(s) if s.success() => self.set_status("Neovim closed"),
                            Ok(s) => self.set_status(format!("nvim exit {:?}", s.code())),
                            Err(e) => self.set_status(format!("nvim: {e:#}")),
                        }
                    }
                    AppEffect::KittyIcat { url } => {
                        ratatui::restore();
                        let tried = std::process::Command::new("kitten")
                            .args(["icat", "--transfer-mode=memory", &url])
                            .status();
                        match tried {
                            Ok(s) if s.success() => {}
                            _ => {
                                let _ = open::that(url.as_str());
                            }
                        }
                        *terminal = ratatui::try_init().context("terminal re-init after icat")?;
                        self.set_status("image (Kitty icat or browser fallback)");
                    }
                    AppEffect::ViewInEditor { text, ext } => {
                        ratatui::restore();
                        let _ = editor::view_text(&text, ext);
                        *terminal = ratatui::try_init().context("terminal re-init after editor")?;
                        self.set_status("closed editor buffer");
                    }
                    AppEffect::None => {}
                },
                Event::Mouse(m) => {
                    let _ = self.handle_mouse(m, rt);
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn set_status(&mut self, s: impl ToString) {
        self.status = s.to_string();
    }

    pub(crate) fn filters_summary(&self) -> String {
        let f = &self.pr_filters;
        let mut lines: Vec<String> = Vec::new();
        if github::pr_list_uses_search(f, self.pr_status) {
            lines.push("Mode: GitHub search (is:pr …)".into());
            lines.push(format!("  q = {}", github::build_pr_search_query(
                &self.owner,
                &self.repo,
                self.pr_status,
                f,
            )));
        } else {
            lines.push("Mode: REST GET /repos/…/pulls".into());
        }
        let mut kv = |k: &str, v: &Option<String>| {
            if let Some(s) = v {
                let t = s.trim();
                if !t.is_empty() {
                    lines.push(format!("  {k}: {t}"));
                }
            }
        };
        kv("head", &f.head);
        kv("base", &f.base);
        kv("author", &f.author);
        kv("assignee", &f.assignee);
        kv("mentions", &f.mentions);
        kv("review-requested", &f.review_requested);
        kv("reviewed-by", &f.reviewed_by);
        kv("label", &f.label);
        kv("title", &f.title_search);
        lines.push(format!("  status: {}", self.pr_status.label()));
        if !self.pr_filters.any_field_set() {
            lines.push("  (no field filters yet)".into());
        }
        lines.push(String::new());
        lines.push(
            "Commands: :filter clear  :author LOGIN  :assignee LOGIN|none  :mentions LOGIN".into(),
        );
        lines.push(
            "  :reviewer LOGIN  :reviewed LOGIN  :label TEXT  :title TEXT  :head BRANCH  :base BRANCH".into(),
        );
        lines.push(
            "  each has `:field clear`  ·  press s here for status menu  ·  Esc or q closes"
                .into(),
        );
        lines.join("\n")
    }

    fn refresh_pr_list(&mut self, rt: &Runtime) -> anyhow::Result<()> {
        self.loading = true;
        let res = rt.block_on(github::fetch_pr_list(
            &self.octo,
            &self.owner,
            &self.repo,
            self.pr_status,
            1,
            github::PR_LIST_PER_PAGE,
            &self.pr_filters,
        ));
        self.loading = false;
        match res {
            Ok(github::PrListPage::Pulls(page)) => {
                self.pr_entries = page.items.into_iter().map(PrListEntry::Rest).collect();
                self.pr_list_total_count = None;
                self.pr_list_page = 1;
                self.pr_list_has_more = page.next.is_some();
                self.pr_cursor = 0;
                if self.me.is_none() {
                    self.me = rt.block_on(github::current_login(&self.octo)).ok().flatten();
                }
                self.set_status(format!(
                    "Page 1 — {} PR(s) (REST, {}){}  ·  m more  r refresh  f filters",
                    self.pr_entries.len(),
                    self.pr_status.label(),
                    if self.pr_list_has_more {
                        " — more on GitHub"
                    } else {
                        ""
                    },
                ));
            }
            Ok(github::PrListPage::Issues(page)) => {
                self.pr_entries = page
                    .items
                    .into_iter()
                    .filter(|i| i.pull_request.is_some())
                    .map(PrListEntry::Search)
                    .collect();
                self.pr_list_total_count = page.total_count;
                self.pr_list_page = 1;
                self.pr_list_has_more = page.next.is_some();
                self.pr_cursor = 0;
                if self.me.is_none() {
                    self.me = rt.block_on(github::current_login(&self.octo)).ok().flatten();
                }
                let total = self
                    .pr_list_total_count
                    .map(|n| format!(" (~{n} matches)"))
                    .unwrap_or_default();
                self.set_status(format!(
                    "Page 1 — {} PR(s) (search){}  ·  m more  r refresh  f filters",
                    self.pr_entries.len(),
                    total,
                ));
            }
            Err(e) => self.set_status(format!("Error loading PRs: {e:#}")),
        }
        Ok(())
    }

    fn load_more_prs(&mut self, rt: &Runtime) -> anyhow::Result<()> {
        if !self.pr_list_has_more {
            self.set_status("No more pages (end of list).");
            return Ok(());
        }
        self.loading = true;
        let next = self.pr_list_page.saturating_add(1);
        let res = rt.block_on(github::fetch_pr_list(
            &self.octo,
            &self.owner,
            &self.repo,
            self.pr_status,
            next,
            github::PR_LIST_PER_PAGE,
            &self.pr_filters,
        ));
        self.loading = false;
        match res {
            Ok(github::PrListPage::Pulls(page)) => {
                let added = page.items.len();
                self.pr_entries
                    .extend(page.items.into_iter().map(PrListEntry::Rest));
                self.pr_list_page = next;
                self.pr_list_has_more = page.next.is_some();
                self.set_status(format!(
                    "Loaded page {} (+{added}) — {} PR(s){}",
                    self.pr_list_page,
                    self.pr_entries.len(),
                    if self.pr_list_has_more {
                        " — m for next page"
                    } else {
                        " — end of list"
                    },
                ));
            }
            Ok(github::PrListPage::Issues(page)) => {
                let next_items: Vec<PrListEntry> = page
                    .items
                    .into_iter()
                    .filter(|i| i.pull_request.is_some())
                    .map(PrListEntry::Search)
                    .collect();
                let added = next_items.len();
                self.pr_entries.extend(next_items);
                self.pr_list_page = next;
                self.pr_list_has_more = page.next.is_some();
                if self.pr_list_total_count.is_none() {
                    self.pr_list_total_count = page.total_count;
                }
                self.set_status(format!(
                    "Loaded page {} (+{added}) — {} PR(s){}",
                    self.pr_list_page,
                    self.pr_entries.len(),
                    if self.pr_list_has_more {
                        " — m for next page"
                    } else {
                        " — end of list"
                    },
                ));
            }
            Err(e) => self.set_status(format!("Load more failed: {e:#}")),
        }
        Ok(())
    }

    fn open_pr(&mut self, rt: &Runtime, number: u64) -> anyhow::Result<()> {
        self.loading = true;
        let pr = rt.block_on(github::get_pull(
            &self.octo,
            &self.owner,
            &self.repo,
            number,
        ));
        self.loading = false;
        match pr {
            Ok(p) => {
                self.screen = Screen::PrDetail;
                self.pr_number = Some(number);
                self.current_pr = Some(p);
                self.pr_tab = PrTab::Thread;
                self.clear_detail_views();
                self.load_thread(rt)?;
                self.prefetch_thread_selection();
                self.set_status(
                    "1-6 tabs  E $EDITOR  V nvim+diff  I image  Thread: j/k c e R + L  r q",
                );
            }
            Err(e) => self.set_status(format!("open PR: {e:#}")),
        }
        Ok(())
    }

    fn clear_detail_views(&mut self) {
        self.thread_items.clear();
        self.thread_cursor = 0;
        self.thread_scroll = 0;
        self.thread_detail_scroll = 0;
        self.thread_hunk_scroll = 0;
        self.reactions_line = None;
        self.reactions_cache.clear();
        self.thread_image_url = None;
        self.commits.clear();
        self.commit_cursor = 0;
        self.files_lines.clear();
        self.file_paths.clear();
        self.file_cursor = 0;
        self.diff_text.clear();
        self.diff_scroll = 0;
        self.reviews_lines.clear();
        self.review_cursor = 0;
        self.thread_list_state = ListState::default();
        self.review_list_state = ListState::default();
    }

    fn load_thread(&mut self, rt: &Runtime) -> anyhow::Result<()> {
        let Some(n) = self.pr_number else { return Ok(()); };
        self.loading = true;
        self.reactions_cache.clear();
        let oct = self.octo.clone();
        let owner = self.owner.clone();
        let repo = self.repo.clone();
        let (issue, review) = rt.block_on(async {
            tokio::join!(
                github::list_issue_comments(&oct, &owner, &repo, n),
                github::list_review_comments(&oct, &owner, &repo, n),
            )
        });
        self.loading = false;
        self.reactions_line = None;
        self.thread_image_url = None;
        let mut items = Vec::new();
        if let Ok(cs) = issue {
            for c in cs {
                let author = c.user.login.clone();
                let body = c.body.clone().unwrap_or_default();
                items.push(ThreadItem::Issue {
                    id: c.id,
                    author,
                    body,
                    created: c.created_at,
                });
            }
        }
        if let Ok(cs) = review {
            for c in cs {
                let author = c.user.as_ref().map(|u| u.login.clone()).unwrap_or_default();
                items.push(ThreadItem::Review {
                    id: c.id,
                    author,
                    body: c.body.clone(),
                    path: c.path.clone(),
                    line: c.line,
                    diff_hunk: c.diff_hunk.clone(),
                    in_reply_to: c.in_reply_to_id,
                    created: c.created_at,
                });
            }
        }
        items.sort_by_key(|i| i.created());
        self.thread_items = items;
        if self.thread_cursor >= self.thread_items.len() && !self.thread_items.is_empty() {
            self.thread_cursor = self.thread_items.len() - 1;
        }
        self.prefetch_thread_selection();
        Ok(())
    }

    /// Fast path for j/k: no network. Use `L` to load reactions (cached per comment).
    fn prefetch_thread_selection(&mut self) {
        self.thread_detail_scroll = 0;
        self.thread_hunk_scroll = 0;
        if self.pr_tab != PrTab::Thread {
            return;
        }
        self.thread_image_url = self
            .selected_thread_item()
            .and_then(|t| markdown_render::first_image_url(t.body()));
        self.reactions_line = self
            .selected_thread_item()
            .map(|it| it.id())
            .and_then(|id| self.reactions_cache.get(&id).cloned());
    }

    fn open_diff_in_nvim(&mut self, rt: &Runtime) -> anyhow::Result<AppEffect> {
        let Some(n) = self.pr_number else {
            return Ok(AppEffect::None);
        };
        if self.diff_text.is_empty() {
            self.load_diff(rt)?;
        }
        if self.thread_items.is_empty() {
            self.load_thread(rt)?;
        }
        if self.diff_text.is_empty() {
            self.set_status("no diff loaded");
            return Ok(AppEffect::None);
        }
        let path = diff_nvim::write_pr_review_nvim_buffer(
            n,
            &self.owner,
            &self.repo,
            &self.diff_text,
            &self.thread_items,
        )?;
        Ok(AppEffect::OpenNvim { path })
    }

    fn load_commits(&mut self, rt: &Runtime) -> anyhow::Result<()> {
        let Some(n) = self.pr_number else { return Ok(()); };
        self.loading = true;
        let r = rt.block_on(github::list_pr_commits(
            &self.octo,
            &self.owner,
            &self.repo,
            n,
        ));
        self.loading = false;
        match r {
            Ok(v) => {
                self.commits = v;
                self.set_status(format!("{} commits", self.commits.len()));
            }
            Err(e) => self.set_status(format!("commits: {e:#}")),
        }
        Ok(())
    }

    fn load_files(&mut self, rt: &Runtime) -> anyhow::Result<()> {
        let Some(n) = self.pr_number else { return Ok(()); };
        self.loading = true;
        let r = rt.block_on(github::list_pr_files(
            &self.octo,
            &self.owner,
            &self.repo,
            n,
        ));
        self.loading = false;
        match r {
            Ok(entries) => {
                self.file_paths = entries.iter().map(|e| e.filename.clone()).collect();
                self.files_lines = entries
                    .into_iter()
                    .map(|e| {
                        format!(
                            "+{} -{}  {:?}  {}",
                            e.additions, e.deletions, e.status, e.filename
                        )
                    })
                    .collect();
                self.set_status(format!("{} files", self.files_lines.len()));
            }
            Err(e) => self.set_status(format!("files: {e:#}")),
        }
        Ok(())
    }

    fn load_diff(&mut self, rt: &Runtime) -> anyhow::Result<()> {
        let Some(n) = self.pr_number else { return Ok(()); };
        self.loading = true;
        let r = rt.block_on(github::get_pr_diff(
            &self.octo,
            &self.owner,
            &self.repo,
            n,
        ));
        self.loading = false;
        match r {
            Ok(mut s) => {
                const MAX: usize = 256 * 1024;
                if s.len() > MAX {
                    s.truncate(MAX);
                    s.push_str("\n\n… truncated (diff too large) …");
                }
                self.diff_text = s;
                self.set_status("diff loaded (scroll with Ctrl-d / Ctrl-u)");
            }
            Err(e) => self.set_status(format!("diff: {e:#}")),
        }
        Ok(())
    }

    fn load_reviews(&mut self, rt: &Runtime) -> anyhow::Result<()> {
        let Some(n) = self.pr_number else { return Ok(()); };
        self.loading = true;
        let r = rt.block_on(github::list_reviews(
            &self.octo,
            &self.owner,
            &self.repo,
            n,
        ));
        self.loading = false;
        match r {
            Ok(rs) => {
                self.reviews_lines = rs
                    .into_iter()
                    .map(|rev| {
                        let who = rev
                            .user
                            .as_ref()
                            .map(|u| u.login.as_str())
                            .unwrap_or("?");
                        let st = rev
                            .state
                            .map(|s| format!("{s:?}"))
                            .unwrap_or_else(|| "?".into());
                        let body = rev.body.unwrap_or_default();
                        format!("{who}  [{st}]  {}", body.lines().next().unwrap_or(""))
                    })
                    .collect();
                self.set_status(format!("{} reviews", self.reviews_lines.len()));
            }
            Err(e) => self.set_status(format!("reviews: {e:#}")),
        }
        Ok(())
    }

    fn ensure_tab_loaded(&mut self, rt: &Runtime) -> anyhow::Result<()> {
        match self.pr_tab {
            PrTab::Info => {}
            PrTab::Thread if self.thread_items.is_empty() => self.load_thread(rt)?,
            PrTab::Commits if self.commits.is_empty() => self.load_commits(rt)?,
            PrTab::Files if self.files_lines.is_empty() => self.load_files(rt)?,
            PrTab::Diff if self.diff_text.is_empty() => self.load_diff(rt)?,
            PrTab::Reviews if self.reviews_lines.is_empty() => self.load_reviews(rt)?,
            _ => {}
        }
        Ok(())
    }

    fn selected_thread_item(&self) -> Option<&ThreadItem> {
        self.thread_items.get(self.thread_cursor)
    }

    fn author_is_me(&self, author: &str) -> bool {
        self.me.as_deref() == Some(author)
    }

    pub fn apply_editor_result(
        &mut self,
        intent: EditorIntent,
        result: anyhow::Result<String>,
        rt: &Runtime,
    ) -> anyhow::Result<()> {
        let body = match result {
            Ok(s) => {
                let t = s.trim();
                if t.is_empty() {
                    self.set_status("empty; cancelled");
                    return Ok(());
                }
                t.to_string()
            }
            Err(e) => {
                self.set_status(format!("editor: {e:#}"));
                return Ok(());
            }
        };
        match intent {
            EditorIntent::NewComment { pr } => {
                let r = rt.block_on(github::create_issue_comment(
                    &self.octo,
                    &self.owner,
                    &self.repo,
                    pr,
                    &body,
                ));
                match r {
                    Ok(_) => {
                        self.set_status("comment posted");
                        let _ = self.load_thread(rt);
                    }
                    Err(e) => self.set_status(format!("post: {e:#}")),
                }
            }
            EditorIntent::Reply { pr, to } => {
                let r = rt.block_on(github::reply_to_review_comment(
                    &self.octo,
                    &self.owner,
                    &self.repo,
                    pr,
                    to,
                    &body,
                ));
                match r {
                    Ok(_) => {
                        self.set_status("reply posted");
                        let _ = self.load_thread(rt);
                    }
                    Err(e) => self.set_status(format!("reply: {e:#}")),
                }
            }
            EditorIntent::EditIssue { id } => {
                let r = rt.block_on(github::update_issue_comment(
                    &self.octo,
                    &self.owner,
                    &self.repo,
                    id,
                    &body,
                ));
                match r {
                    Ok(_) => {
                        self.set_status("updated");
                        let _ = self.load_thread(rt);
                    }
                    Err(e) => self.set_status(format!("update: {e:#}")),
                }
            }
            EditorIntent::EditReview { id } => {
                let r = rt.block_on(github::update_review_comment(
                    &self.octo,
                    &self.owner,
                    &self.repo,
                    id,
                    &body,
                ));
                match r {
                    Ok(_) => {
                        self.set_status("updated");
                        let _ = self.load_thread(rt);
                    }
                    Err(e) => self.set_status(format!("update: {e:#}")),
                }
            }
            EditorIntent::CreatePull {
                title,
                head,
                base,
            } => {
                let r = rt.block_on(github::create_pull(
                    &self.octo,
                    &self.owner,
                    &self.repo,
                    &title,
                    &head,
                    &base,
                    Some(&body),
                    false,
                ));
                match r {
                    Ok(pr) => {
                        self.set_status(format!("created PR #{}", pr.number));
                        self.overlay = Overlay::None;
                        let _ = self.refresh_pr_list(rt);
                    }
                    Err(e) => self.set_status(format!("create PR: {e:#}")),
                }
            }
            EditorIntent::InlineReviewComment {
                pr,
                commit_sha,
                path,
                line,
                side,
            } => {
                let r = rt.block_on(github::create_pull_review_inline_comment(
                    &self.octo,
                    &self.owner,
                    &self.repo,
                    pr,
                    &commit_sha,
                    &path,
                    line,
                    &side,
                    &body,
                ));
                match r {
                    Ok(_) => {
                        self.set_status("posted inline review comment");
                        self.overlay = Overlay::None;
                        let _ = self.load_thread(rt);
                        let _ = self.load_reviews(rt);
                    }
                    Err(e) => self.set_status(format!("inline comment: {e:#}")),
                }
            }
        }
        Ok(())
    }

    pub fn handle_key(&mut self, key: KeyEvent, rt: &Runtime) -> anyhow::Result<AppEffect> {
        if key.kind != crossterm::event::KeyEventKind::Press {
            return Ok(AppEffect::None);
        }

        if !matches!(self.overlay, Overlay::None) {
            return self.handle_overlay_key(key, rt);
        }

        match self.screen {
            Screen::PrList => self.handle_pr_list(key, rt),
            Screen::PrDetail => self.handle_pr_detail(key, rt),
        }
    }

    fn handle_overlay_key(&mut self, key: KeyEvent, rt: &Runtime) -> anyhow::Result<AppEffect> {
        match &mut self.overlay {
            Overlay::Help => {
                if matches!(key.code, KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?')) {
                    self.overlay = Overlay::None;
                }
                Ok(AppEffect::None)
            }
            Overlay::FilterSummary(phase) => match phase {
                FilterPanelPhase::Overview => match key.code {
                    KeyCode::Esc | KeyCode::Char('q') => {
                        self.overlay = Overlay::None;
                        Ok(AppEffect::None)
                    }
                    KeyCode::Char('s') | KeyCode::Char('S') => {
                        self.overlay = Overlay::FilterSummary(FilterPanelPhase::StatusPick {
                            cursor: pr_status_menu_cursor(self.pr_status),
                        });
                        Ok(AppEffect::None)
                    }
                    _ => {
                        self.overlay = Overlay::None;
                        Ok(AppEffect::None)
                    }
                },
                FilterPanelPhase::StatusPick { cursor } => match key.code {
                    KeyCode::Esc => {
                        self.overlay = Overlay::FilterSummary(FilterPanelPhase::Overview);
                        Ok(AppEffect::None)
                    }
                    KeyCode::Char('q') => {
                        self.overlay = Overlay::None;
                        Ok(AppEffect::None)
                    }
                    KeyCode::Char('j') | KeyCode::Down => {
                        *cursor = (*cursor + 1).min(4);
                        Ok(AppEffect::None)
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        *cursor = cursor.saturating_sub(1);
                        Ok(AppEffect::None)
                    }
                    KeyCode::Enter => {
                        self.pr_status = match *cursor {
                            0 => github::PrStatusFilter::Open,
                            1 => github::PrStatusFilter::Closed,
                            2 => github::PrStatusFilter::Merged,
                            3 => github::PrStatusFilter::Draft,
                            _ => github::PrStatusFilter::All,
                        };
                        self.overlay = Overlay::None;
                        let _ = self.refresh_pr_list(rt);
                        Ok(AppEffect::None)
                    }
                    _ => Ok(AppEffect::None),
                },
            },
            Overlay::Command => match key.code {
                KeyCode::Esc => {
                    self.command_buf.clear();
                    self.overlay = Overlay::None;
                    Ok(AppEffect::None)
                }
                KeyCode::Enter => {
                    let cmd = std::mem::take(&mut self.command_buf);
                    self.overlay = Overlay::None;
                    if let Some(e) = self.run_command(&cmd, rt)? {
                        return Ok(e);
                    }
                    Ok(AppEffect::None)
                }
                KeyCode::Char(c) => {
                    self.command_buf.push(c);
                    Ok(AppEffect::None)
                }
                KeyCode::Backspace => {
                    self.command_buf.pop();
                    Ok(AppEffect::None)
                }
                _ => Ok(AppEffect::None),
            },
            Overlay::ReactionPicker => {
                let opts = reaction_options();
                match key.code {
                    KeyCode::Esc => {
                        self.overlay = Overlay::None;
                    }
                    KeyCode::Char('j') | KeyCode::Down => {
                        self.reaction_cursor = (self.reaction_cursor + 1).min(opts.len().saturating_sub(1));
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        self.reaction_cursor = self.reaction_cursor.saturating_sub(1);
                    }
                    KeyCode::Enter => {
                        if let Some(content) = opts.get(self.reaction_cursor).cloned() {
                            self.apply_reaction(content, rt)?;
                        }
                        self.overlay = Overlay::None;
                    }
                    _ => {}
                }
                Ok(AppEffect::None)
            }
            Overlay::InlineReview {
                phase,
                file_cursor,
                path,
                diff_lines,
                line_cursor,
            } => match *phase {
                InlineReviewPhase::PickFile => {
                    match key.code {
                        KeyCode::Esc | KeyCode::Char('q') => {
                            self.overlay = Overlay::None;
                        }
                        KeyCode::Char('j') | KeyCode::Down => {
                            if !self.file_paths.is_empty() {
                                *file_cursor =
                                    (*file_cursor + 1).min(self.file_paths.len() - 1);
                            }
                        }
                        KeyCode::Char('k') | KeyCode::Up => {
                            *file_cursor = file_cursor.saturating_sub(1);
                        }
                        KeyCode::Enter => {
                            if self.file_paths.is_empty() {
                                self.set_status("no files on this PR");
                            } else if let Some(p) = self.file_paths.get(*file_cursor).cloned() {
                                match diff_pick::extract_file_patch(&self.diff_text, &p) {
                                    None => self.set_status(format!("no diff chunk for `{p}`")),
                                    Some(chunk) => {
                                        if chunk.contains("Binary files ")
                                            && chunk.contains(" differ")
                                        {
                                            self.set_status("binary file — pick another");
                                        } else {
                                            let lines = diff_pick::parse_patch_lines(chunk);
                                            if diff_pick::first_anchor_index(&lines).is_none() {
                                                self.set_status(
                                                    "no commentable lines in this diff chunk",
                                                );
                                            } else {
                                                *phase = InlineReviewPhase::PickLine;
                                                *path = p;
                                                *diff_lines = lines;
                                                *line_cursor = diff_pick::first_anchor_index(
                                                    diff_lines,
                                                )
                                                .unwrap_or(0);
                                                self.inline_review_line_state =
                                                    ListState::default();
                                                self.set_status(
                                                    "line → Enter $EDITOR · n/p anchors · Esc file list · q quit",
                                                );
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                    Ok(AppEffect::None)
                }
                InlineReviewPhase::PickLine => match key.code {
                    KeyCode::Esc => {
                        *phase = InlineReviewPhase::PickFile;
                        diff_lines.clear();
                        path.clear();
                        self.inline_review_line_state = ListState::default();
                        self.set_status("pick a file (Enter)");
                        Ok(AppEffect::None)
                    }
                    KeyCode::Char('q') => {
                        self.overlay = Overlay::None;
                        Ok(AppEffect::None)
                    }
                    KeyCode::Char('j') | KeyCode::Down => {
                        if !diff_lines.is_empty() {
                            *line_cursor = (*line_cursor + 1).min(diff_lines.len() - 1);
                        }
                        Ok(AppEffect::None)
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        *line_cursor = line_cursor.saturating_sub(1);
                        Ok(AppEffect::None)
                    }
                    KeyCode::Char('n') => {
                        if !diff_lines.is_empty() {
                            *line_cursor =
                                diff_pick::step_anchor(*line_cursor, diff_lines, true);
                        }
                        Ok(AppEffect::None)
                    }
                    KeyCode::Char('p') => {
                        if !diff_lines.is_empty() {
                            *line_cursor =
                                diff_pick::step_anchor(*line_cursor, diff_lines, false);
                        }
                        Ok(AppEffect::None)
                    }
                    KeyCode::Enter => {
                        let Some(n) = self.pr_number else {
                            return Ok(AppEffect::None);
                        };
                        let Some(pr_obj) = self.current_pr.as_ref() else {
                            return Ok(AppEffect::None);
                        };
                        if let Some(dl) = diff_lines.get(*line_cursor) {
                            if let Some((line, side)) = dl.anchor {
                                let initial = format!(
                                    "## `{path}` line {line} ({side})\n\n\
Comment (markdown). Optional suggestion block:\n\n\
```suggestion\n\n```\n",
                                    path = path.as_str(),
                                    line = line,
                                    side = side,
                                );
                                return Ok(AppEffect::OpenEditor {
                                    initial,
                                    intent: EditorIntent::InlineReviewComment {
                                        pr: n,
                                        commit_sha: pr_obj.head.sha.clone(),
                                        path: path.clone(),
                                        line,
                                        side: side.to_string(),
                                    },
                                });
                            }
                        }
                        self.set_status("select a numbered diff row (n / p jump commentable lines)");
                        Ok(AppEffect::None)
                    }
                    _ => Ok(AppEffect::None),
                },
            },
            Overlay::CreatePrWizard {
                phase,
                title,
                head,
                base,
                buf,
            } => match key.code {
                KeyCode::Esc => {
                    self.overlay = Overlay::None;
                    Ok(AppEffect::None)
                }
                KeyCode::Enter => {
                    match *phase {
                        0 => {
                            *title = std::mem::take(buf);
                            if title.is_empty() {
                                self.set_status("title required");
                            } else {
                                *phase = 1;
                            }
                        }
                        1 => {
                            *head = std::mem::take(buf);
                            if head.is_empty() {
                                self.set_status("head branch required");
                            } else {
                                *phase = 2;
                            }
                        }
                        2 => {
                            *base = std::mem::take(buf);
                            if base.is_empty() {
                                self.set_status("base branch required");
                            } else {
                                let t = title.clone();
                                let h = head.clone();
                                let b = base.clone();
                                self.overlay = Overlay::None;
                                return Ok(AppEffect::OpenEditor {
                                    initial: String::new(),
                                    intent: EditorIntent::CreatePull {
                                        title: t,
                                        head: h,
                                        base: b,
                                    },
                                });
                            }
                        }
                        _ => {}
                    }
                    Ok(AppEffect::None)
                }
                KeyCode::Backspace => {
                    buf.pop();
                    Ok(AppEffect::None)
                }
                KeyCode::Char(c) => {
                    buf.push(c);
                    Ok(AppEffect::None)
                }
                _ => Ok(AppEffect::None),
            },
            Overlay::None => Ok(AppEffect::None),
        }
    }

    fn run_command(&mut self, cmd: &str, rt: &Runtime) -> anyhow::Result<Option<AppEffect>> {
        let parts: Vec<&str> = cmd.trim().split_whitespace().collect();
        if parts.is_empty() {
            return Ok(None);
        }
        match parts[0] {
            "q" | "quit" | "exit" => return Ok(Some(AppEffect::Quit)),
            "repo" if parts.len() == 2 => {
                if let Some((o, r)) = parts[1].split_once('/') {
                    self.owner = o.to_string();
                    self.repo = r.to_string();
                    self.set_status(format!("repo → {}/{}", self.owner, self.repo));
                    let _ = self.refresh_pr_list(rt);
                } else {
                    self.set_status("usage: repo owner/name");
                }
            }
            "state" if parts.len() == 2 => {
                self.pr_status = match parts[1] {
                    "open" => github::PrStatusFilter::Open,
                    "closed" => github::PrStatusFilter::Closed,
                    "merged" => github::PrStatusFilter::Merged,
                    "draft" => github::PrStatusFilter::Draft,
                    "all" => github::PrStatusFilter::All,
                    _ => {
                        self.set_status("state open|closed|merged|draft|all");
                        return Ok(None);
                    }
                };
                let _ = self.refresh_pr_list(rt);
            }
            "title" => {
                if parts.len() < 2 {
                    self.set_status(":title TEXT (in:title search) | :title clear");
                } else if parts[1] == "clear" {
                    self.pr_filters.title_search = None;
                    let _ = self.refresh_pr_list(rt);
                } else {
                    self.pr_filters.title_search = Some(parts[1..].join(" "));
                    let _ = self.refresh_pr_list(rt);
                }
            }
            "merge" if self.screen == Screen::PrDetail => {
                let method = match parts.get(1).copied().unwrap_or("merge") {
                    "squash" => MergeMethod::Squash,
                    "rebase" => MergeMethod::Rebase,
                    _ => MergeMethod::Merge,
                };
                if let Some(n) = self.pr_number {
                    let r = rt.block_on(github::merge_pull(
                        &self.octo,
                        &self.owner,
                        &self.repo,
                        n,
                        method,
                    ));
                    match r {
                        Ok(()) => self.set_status("merge requested"),
                        Err(e) => self.set_status(format!("merge: {e:#}")),
                    }
                }
            }
            "update-branch" | "ub" if self.screen == Screen::PrDetail => {
                if let Some(n) = self.pr_number {
                    let r = rt.block_on(github::update_pr_branch(
                        &self.octo,
                        &self.owner,
                        &self.repo,
                        n,
                    ));
                    match r {
                        Ok(b) => self.set_status(format!("update branch: {b}")),
                        Err(e) => self.set_status(format!("update-branch: {e:#}")),
                    }
                }
            }
            "filter" => {
                if parts.len() < 2 {
                    self.set_status("filter clear | filter show");
                } else {
                    match parts[1] {
                        "clear" => {
                            self.pr_filters = github::PrListFilters::default();
                            let _ = self.refresh_pr_list(rt);
                        }
                        "show" => {
                            self.overlay = Overlay::FilterSummary(FilterPanelPhase::Overview);
                        }
                        _ => self.set_status("filter clear | filter show"),
                    }
                }
            }
            "author" => {
                if parts.len() < 2 {
                    self.set_status(":author LOGIN | :author clear");
                } else if parts[1] == "clear" {
                    self.pr_filters.author = None;
                    let _ = self.refresh_pr_list(rt);
                } else {
                    self.pr_filters.author = Some(parts[1..].join(" "));
                    let _ = self.refresh_pr_list(rt);
                }
            }
            "assignee" => {
                if parts.len() < 2 {
                    self.set_status(":assignee LOGIN | none | :assignee clear");
                } else if parts[1] == "clear" {
                    self.pr_filters.assignee = None;
                    let _ = self.refresh_pr_list(rt);
                } else {
                    self.pr_filters.assignee = Some(parts[1..].join(" "));
                    let _ = self.refresh_pr_list(rt);
                }
            }
            "mentions" => {
                if parts.len() < 2 {
                    self.set_status(":mentions LOGIN | :mentions clear");
                } else if parts[1] == "clear" {
                    self.pr_filters.mentions = None;
                    let _ = self.refresh_pr_list(rt);
                } else {
                    self.pr_filters.mentions = Some(parts[1..].join(" "));
                    let _ = self.refresh_pr_list(rt);
                }
            }
            "reviewer" => {
                if parts.len() < 2 {
                    self.set_status(":reviewer LOGIN (review-requested) | :reviewer clear");
                } else if parts[1] == "clear" {
                    self.pr_filters.review_requested = None;
                    let _ = self.refresh_pr_list(rt);
                } else {
                    self.pr_filters.review_requested = Some(parts[1..].join(" "));
                    let _ = self.refresh_pr_list(rt);
                }
            }
            "reviewed" => {
                if parts.len() < 2 {
                    self.set_status(":reviewed LOGIN (reviewed-by) | :reviewed clear");
                } else if parts[1] == "clear" {
                    self.pr_filters.reviewed_by = None;
                    let _ = self.refresh_pr_list(rt);
                } else {
                    self.pr_filters.reviewed_by = Some(parts[1..].join(" "));
                    let _ = self.refresh_pr_list(rt);
                }
            }
            "label" => {
                if parts.len() < 2 {
                    self.set_status(":label NAME | :label clear");
                } else if parts[1] == "clear" {
                    self.pr_filters.label = None;
                    let _ = self.refresh_pr_list(rt);
                } else {
                    self.pr_filters.label = Some(parts[1..].join(" "));
                    let _ = self.refresh_pr_list(rt);
                }
            }
            "head" => {
                if parts.len() < 2 {
                    self.set_status(":head BRANCH | :head clear");
                } else if parts[1] == "clear" {
                    self.pr_filters.head = None;
                    let _ = self.refresh_pr_list(rt);
                } else {
                    self.pr_filters.head = Some(parts[1..].join(" "));
                    let _ = self.refresh_pr_list(rt);
                }
            }
            "base" => {
                if parts.len() < 2 {
                    self.set_status(":base BRANCH | :base clear");
                } else if parts[1] == "clear" {
                    self.pr_filters.base = None;
                    let _ = self.refresh_pr_list(rt);
                } else {
                    self.pr_filters.base = Some(parts[1..].join(" "));
                    let _ = self.refresh_pr_list(rt);
                }
            }
            "create" | "pr" => {
                let (head, base) = git::pr_wizard_defaults();
                self.overlay = Overlay::CreatePrWizard {
                    phase: 0,
                    title: String::new(),
                    head,
                    base,
                    buf: String::new(),
                };
            }
            "help" => {
                self.overlay = Overlay::Help;
            }
            "more" if self.screen == Screen::PrList => {
                let _ = self.load_more_prs(rt);
            }
            _ => self.set_status(format!("unknown command: {}", parts.join(" "))),
        }
        Ok(None)
    }

    fn apply_reaction(&mut self, content: ReactionContent, rt: &Runtime) -> anyhow::Result<()> {
        let Some(item) = self.selected_thread_item().cloned() else {
            return Ok(());
        };
        let cid = item.id();
        let r = match &item {
            ThreadItem::Issue { id, .. } => {
                rt.block_on(github::create_issue_comment_reaction(
                    &self.octo,
                    &self.owner,
                    &self.repo,
                    *id,
                    content,
                ))
            }
            ThreadItem::Review { id, .. } => {
                rt.block_on(github::create_pull_comment_reaction(
                    &self.octo,
                    &self.owner,
                    &self.repo,
                    *id,
                    content,
                ))
            }
        };
        match r {
            Ok(_) => {
                self.set_status("reaction added — L to refresh counts");
                self.reactions_cache.remove(&cid);
                self.reactions_line = None;
            }
            Err(e) => self.set_status(format!("reaction: {e:#}")),
        }
        Ok(())
    }

    fn handle_pr_list(&mut self, key: KeyEvent, rt: &Runtime) -> anyhow::Result<AppEffect> {
        match key.code {
            KeyCode::Char('q') => return Ok(AppEffect::Quit),
            KeyCode::Char('?') => {
                self.overlay = Overlay::Help;
            }
            KeyCode::Char(':') => {
                self.overlay = Overlay::Command;
                self.command_buf.clear();
            }
            KeyCode::Char('r') => {
                let _ = self.refresh_pr_list(rt);
            }
            KeyCode::Char('m') => {
                let _ = self.load_more_prs(rt);
            }
            KeyCode::Char('a') => {
                self.pr_status = match self.pr_status {
                    github::PrStatusFilter::Open => github::PrStatusFilter::Closed,
                    github::PrStatusFilter::Closed => github::PrStatusFilter::Merged,
                    github::PrStatusFilter::Merged => github::PrStatusFilter::Draft,
                    github::PrStatusFilter::Draft => github::PrStatusFilter::All,
                    github::PrStatusFilter::All => github::PrStatusFilter::Open,
                };
                let _ = self.refresh_pr_list(rt);
            }
            KeyCode::Char('n') => {
                let (head, base) = git::pr_wizard_defaults();
                self.overlay = Overlay::CreatePrWizard {
                    phase: 0,
                    title: String::new(),
                    head,
                    base,
                    buf: String::new(),
                };
            }
            KeyCode::Char('j') | KeyCode::Down => {
                if !self.pr_entries.is_empty() {
                    self.pr_cursor = (self.pr_cursor + 1).min(self.pr_entries.len() - 1);
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.pr_cursor = self.pr_cursor.saturating_sub(1);
            }
            KeyCode::Char('g') => {
                if self.vim_g_pending {
                    self.pr_cursor = 0;
                    self.vim_g_pending = false;
                } else {
                    self.vim_g_pending = true;
                }
            }
            KeyCode::Char('G') => {
                if !self.pr_entries.is_empty() {
                    self.pr_cursor = self.pr_entries.len() - 1;
                }
            }
            KeyCode::Enter => {
                if let Some(e) = self.pr_entries.get(self.pr_cursor) {
                    let n = e.number();
                    let _ = self.open_pr(rt, n);
                }
            }
            KeyCode::Char('f') => {
                self.overlay = Overlay::FilterSummary(FilterPanelPhase::Overview);
            }
            KeyCode::Char('A') => {
                self.overlay = Overlay::FilterSummary(FilterPanelPhase::StatusPick {
                    cursor: pr_status_menu_cursor(self.pr_status),
                });
            }
            KeyCode::Char('o') => {
                if let Some(e) = self.pr_entries.get(self.pr_cursor) {
                    if let Some(u) = e.html_url_open() {
                        let _ = open::that(u.as_str());
                    }
                }
            }
            _ => {
                self.vim_g_pending = false;
            }
        }
        Ok(AppEffect::None)
    }

    fn handle_pr_detail(&mut self, key: KeyEvent, rt: &Runtime) -> anyhow::Result<AppEffect> {
        let _ = self.ensure_tab_loaded(rt);

        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('d') => self.page_down_current(),
                KeyCode::Char('u') => self.page_up_current(),
                _ => {}
            }
            return Ok(AppEffect::None);
        }

        match key.code {
            KeyCode::Char('q') => {
                self.screen = Screen::PrList;
                self.pr_number = None;
                self.current_pr = None;
                self.clear_detail_views();
                self.set_status(format!(
                    "PR list — {} loaded (page {}, {})  ·  m more  r refresh  f filters",
                    self.pr_entries.len(),
                    self.pr_list_page,
                    if self.pr_list_has_more {
                        "more available"
                    } else {
                        "end of list"
                    },
                ));
            }
            KeyCode::Char('?') => {
                self.overlay = Overlay::Help;
            }
            KeyCode::Char(':') => {
                self.overlay = Overlay::Command;
                self.command_buf.clear();
            }
            KeyCode::Char('r') => {
                let n = self.pr_number;
                if let Some(num) = n {
                    let _ = self.open_pr(rt, num);
                }
            }
            KeyCode::Char('j') | KeyCode::Down => self.move_down_detail(rt),
            KeyCode::Char('k') | KeyCode::Up => self.move_up_detail(rt),
            KeyCode::Char('g') => {
                if self.vim_g_pending {
                    self.scroll_top_current();
                    self.vim_g_pending = false;
                } else {
                    self.vim_g_pending = true;
                }
            }
            KeyCode::Char('G') => self.scroll_bottom_current(),
            KeyCode::Char('c') => {
                if self.pr_tab == PrTab::Thread {
                    if let Some(n) = self.pr_number {
                        return Ok(AppEffect::OpenEditor {
                            initial: String::new(),
                            intent: EditorIntent::NewComment { pr: n },
                        });
                    }
                }
            }
            KeyCode::Char('a') => {
                if self.pr_tab == PrTab::Reviews {
                    let _ = self.start_inline_review_wizard(rt)?;
                }
            }
            KeyCode::Char('R') => {
                if self.pr_tab == PrTab::Thread {
                    if let Some(ThreadItem::Review { id, .. }) = self.selected_thread_item() {
                        if let Some(n) = self.pr_number {
                            return Ok(AppEffect::OpenEditor {
                                initial: String::new(),
                                intent: EditorIntent::Reply { pr: n, to: *id },
                            });
                        }
                    } else {
                        self.set_status("select a review comment to reply");
                    }
                }
            }
            KeyCode::Char('e') => {
                if self.pr_tab == PrTab::Thread {
                    if let Some(it) = self.selected_thread_item() {
                        let auth = it.author();
                        if !self.author_is_me(auth) {
                            self.set_status("can only edit your own comments");
                        } else {
                            match it {
                                ThreadItem::Issue { id, body, .. } => {
                                    return Ok(AppEffect::OpenEditor {
                                        initial: body.clone(),
                                        intent: EditorIntent::EditIssue { id: *id },
                                    });
                                }
                                ThreadItem::Review { id, body, .. } => {
                                    return Ok(AppEffect::OpenEditor {
                                        initial: body.clone(),
                                        intent: EditorIntent::EditReview { id: *id },
                                    });
                                }
                            }
                        }
                    }
                }
            }
            KeyCode::Char('+') => {
                if self.pr_tab == PrTab::Thread && self.selected_thread_item().is_some() {
                    self.reaction_cursor = 0;
                    self.overlay = Overlay::ReactionPicker;
                }
            }
            KeyCode::Char('L') => {
                if self.pr_tab == PrTab::Thread {
                    let _ = self.load_reactions_line(rt);
                }
            }
            KeyCode::Char('V') => {
                return self.open_diff_in_nvim(rt);
            }
            KeyCode::Char('E') => {
                if self.pr_tab == PrTab::Diff && self.diff_text.is_empty() {
                    self.load_diff(rt)?;
                }
                if let Some((text, ext)) = self.tab_editor_buffer() {
                    if !text.is_empty() {
                        return Ok(AppEffect::ViewInEditor { text, ext });
                    }
                }
                self.set_status("empty tab — nothing to open in $EDITOR");
            }
            KeyCode::Char('[') => {
                if self.pr_tab == PrTab::Thread {
                    const H: usize = 6;
                    self.thread_hunk_scroll = self.thread_hunk_scroll.saturating_sub(H);
                }
            }
            KeyCode::Char(']') => {
                if self.pr_tab == PrTab::Thread {
                    const H: usize = 6;
                    self.thread_hunk_scroll = self.thread_hunk_scroll.saturating_add(H);
                }
            }
            KeyCode::Char('I') => {
                if self.pr_tab == PrTab::Thread {
                    if let Some(u) = self.thread_image_url.clone() {
                        return Ok(AppEffect::KittyIcat { url: u });
                    }
                    self.set_status("no image in this comment (markdown ![]())");
                }
            }
            KeyCode::Char('d') => {
                if self.pr_tab == PrTab::Thread {
                    if let Some(it) = self.selected_thread_item() {
                        let auth = it.author();
                        if !self.author_is_me(auth) {
                            self.set_status("can only delete your own comments");
                        } else {
                            match it {
                                ThreadItem::Issue { id, .. } => {
                                    let r = rt.block_on(github::delete_issue_comment(
                                        &self.octo,
                                        &self.owner,
                                        &self.repo,
                                        *id,
                                    ));
                                    match r {
                                        Ok(()) => {
                                            self.set_status("deleted");
                                            let _ = self.load_thread(rt);
                                        }
                                        Err(e) => self.set_status(format!("delete: {e:#}")),
                                    }
                                }
                                ThreadItem::Review { id, .. } => {
                                    let r = rt.block_on(github::delete_review_comment(
                                        &self.octo,
                                        &self.owner,
                                        &self.repo,
                                        *id,
                                    ));
                                    match r {
                                        Ok(()) => {
                                            self.set_status("deleted");
                                            let _ = self.load_thread(rt);
                                        }
                                        Err(e) => self.set_status(format!("delete: {e:#}")),
                                    }
                                }
                            }
                        }
                    }
                }
            }
            KeyCode::Char('o') => {
                if let Some(pr) = &self.current_pr {
                    if let Some(u) = pr.html_url.as_ref() {
                        let _ = open::that(u.as_str());
                    }
                }
            }
            KeyCode::Char(c) if matches!(c, '1'..='6') => {
                if let Some(d) = c.to_digit(10) {
                    if let Some(tab) = PrTab::from_digit(d as u8) {
                        self.pr_tab = tab;
                        self.tab_scroll = 0;
                        let _ = self.ensure_tab_loaded(rt);
                    }
                }
            }
            _ => {
                self.vim_g_pending = false;
            }
        }
        Ok(AppEffect::None)
    }

    fn start_inline_review_wizard(&mut self, rt: &Runtime) -> anyhow::Result<()> {
        if self.pr_number.is_none() {
            return Ok(());
        }
        if self.file_paths.is_empty() {
            self.load_files(rt)?;
        }
        if self.diff_text.is_empty() {
            self.load_diff(rt)?;
        }
        if self.file_paths.is_empty() {
            self.set_status("no changed files — cannot comment on diff");
            return Ok(());
        }
        self.inline_review_file_state = ListState::default();
        self.inline_review_line_state = ListState::default();
        self.overlay = Overlay::InlineReview {
            phase: InlineReviewPhase::PickFile,
            file_cursor: 0,
            path: String::new(),
            diff_lines: Vec::new(),
            line_cursor: 0,
        };
        self.set_status("inline review wizard — pick file (Enter) · mouse · Esc/q quit");
        Ok(())
    }

    fn handle_inline_review_mouse(&mut self, m: MouseEvent) {
        let Some(r) = self.wizard_hit_rect.get() else {
            return;
        };
        if m.column < r.x
            || m.column >= r.x.saturating_add(r.width)
            || m.row < r.y
            || m.row >= r.y.saturating_add(r.height)
        {
            return;
        }
        let dy = (m.row - r.y) as usize;
        match &mut self.overlay {
            Overlay::InlineReview {
                phase: InlineReviewPhase::PickFile,
                file_cursor,
                ..
            } => {
                if dy < self.file_paths.len() {
                    *file_cursor = dy;
                }
            }
            Overlay::InlineReview {
                phase: InlineReviewPhase::PickLine,
                line_cursor,
                diff_lines,
                ..
            } => {
                if dy < diff_lines.len() {
                    *line_cursor = dy;
                }
            }
            _ => {}
        }
    }

    fn load_reactions_line(&mut self, rt: &Runtime) -> anyhow::Result<()> {
        let Some(it) = self.selected_thread_item() else {
            return Ok(());
        };
        let cid = it.id();
        let r = match it {
            ThreadItem::Issue { id, .. } => rt.block_on(github::list_issue_comment_reactions(
                &self.octo,
                &self.owner,
                &self.repo,
                *id,
            )),
            ThreadItem::Review { id, .. } => rt.block_on(github::list_pull_comment_reactions(
                &self.octo,
                &self.owner,
                &self.repo,
                *id,
            )),
        };
        match r {
            Ok(rs) => {
                let mut m: HashMap<String, u32> = HashMap::new();
                for x in rs {
                    let k = reaction_kind_label(&x.content).to_string();
                    *m.entry(k).or_insert(0) += 1;
                }
                let s: Vec<String> = m
                    .into_iter()
                    .map(|(a, b)| format!("{a}×{b}"))
                    .collect();
                let line = s.join("  ");
                self.reactions_cache.insert(cid, line.clone());
                self.reactions_line = Some(line);
            }
            Err(e) => self.set_status(format!("reactions: {e:#}")),
        }
        Ok(())
    }

    fn move_down_detail(&mut self, _rt: &Runtime) {
        match self.pr_tab {
            PrTab::Thread => {
                if !self.thread_items.is_empty() {
                    self.thread_cursor = (self.thread_cursor + 1).min(self.thread_items.len() - 1);
                    self.prefetch_thread_selection();
                }
            }
            PrTab::Commits => {
                if !self.commits.is_empty() {
                    self.commit_cursor = (self.commit_cursor + 1).min(self.commits.len() - 1);
                }
            }
            PrTab::Files => {
                if !self.files_lines.is_empty() {
                    self.file_cursor = (self.file_cursor + 1).min(self.files_lines.len() - 1);
                }
            }
            PrTab::Reviews => {
                if !self.reviews_lines.is_empty() {
                    self.review_cursor =
                        (self.review_cursor + 1).min(self.reviews_lines.len() - 1);
                }
            }
            _ => {}
        }
    }

    fn move_up_detail(&mut self, _rt: &Runtime) {
        match self.pr_tab {
            PrTab::Thread => {
                self.thread_cursor = self.thread_cursor.saturating_sub(1);
                self.prefetch_thread_selection();
            }
            PrTab::Commits => {
                self.commit_cursor = self.commit_cursor.saturating_sub(1);
            }
            PrTab::Files => {
                self.file_cursor = self.file_cursor.saturating_sub(1);
            }
            PrTab::Reviews => {
                self.review_cursor = self.review_cursor.saturating_sub(1);
            }
            _ => {}
        }
    }

    fn page_down_current(&mut self) {
        const PAGE: usize = 12;
        match self.pr_tab {
            PrTab::Info => self.tab_scroll += PAGE,
            PrTab::Thread => {
                self.thread_scroll += PAGE;
                self.thread_detail_scroll += PAGE;
            }
            PrTab::Diff => self.diff_scroll += PAGE,
            _ => {}
        }
    }

    fn page_up_current(&mut self) {
        const PAGE: usize = 12;
        match self.pr_tab {
            PrTab::Info => {
                self.tab_scroll = self.tab_scroll.saturating_sub(PAGE);
            }
            PrTab::Thread => {
                self.thread_scroll = self.thread_scroll.saturating_sub(PAGE);
                self.thread_detail_scroll = self.thread_detail_scroll.saturating_sub(PAGE);
            }
            PrTab::Diff => {
                self.diff_scroll = self.diff_scroll.saturating_sub(PAGE);
            }
            _ => {}
        }
    }

    fn scroll_top_current(&mut self) {
        match self.pr_tab {
            PrTab::Info => self.tab_scroll = 0,
            PrTab::Thread => {
                self.thread_scroll = 0;
                self.thread_detail_scroll = 0;
                self.thread_hunk_scroll = 0;
            }
            PrTab::Diff => self.diff_scroll = 0,
            _ => {}
        }
    }

    fn scroll_bottom_current(&mut self) {
        match self.pr_tab {
            PrTab::Diff => {
                self.diff_scroll = self.diff_text.lines().count();
            }
            _ => {}
        }
    }

    fn handle_mouse(&mut self, m: MouseEvent, _rt: &Runtime) -> anyhow::Result<()> {
        if m.kind != MouseEventKind::Down(MouseButton::Left) {
            return Ok(());
        }
        match &self.overlay {
            Overlay::FilterSummary(_) => {
                self.overlay = Overlay::None;
                return Ok(());
            }
            Overlay::InlineReview { .. } => {
                self.handle_inline_review_mouse(m);
                return Ok(());
            }
            Overlay::None => {}
            _ => return Ok(()),
        }
        if self.screen == Screen::PrList {
            if let Some(i) = self.pr_list_row_at_click(m.column, m.row) {
                self.pr_cursor = i;
                if let Some(e) = self.pr_entries.get(i) {
                    self.set_status(format!(
                        "selected #{} — Enter to open  o browser",
                        e.number()
                    ));
                }
            }
        }
        Ok(())
    }

    fn pr_list_row_at_click(&self, col: u16, row: u16) -> Option<usize> {
        let r = self.pr_list_hit_rect.get()?;
        if col < r.x
            || col >= r.x.saturating_add(r.width)
            || row < r.y
            || row >= r.y.saturating_add(r.height)
        {
            return None;
        }
        let dy = (row - r.y) as usize;
        let idx = dy / 2;
        (idx < self.pr_entries.len()).then_some(idx)
    }

    fn tab_editor_buffer(&self) -> Option<(String, &'static str)> {
        match self.pr_tab {
            PrTab::Info => {
                let p = self.current_pr.as_ref()?;
                let t = p.title.as_deref().unwrap_or("(no title)");
                let b = p.body.as_deref().unwrap_or("");
                Some((format!("{t}\n\n{b}"), ".md"))
            }
            PrTab::Thread => self
                .selected_thread_item()
                .map(|it| (Self::thread_detail_plain(it), ".md")),
            PrTab::Diff => (!self.diff_text.is_empty()).then_some((self.diff_text.clone(), ".diff")),
            PrTab::Reviews => (!self.reviews_lines.is_empty())
                .then_some((self.reviews_lines.join("\n"), ".txt")),
            PrTab::Commits => self.commits.get(self.commit_cursor).map(|c| {
                (
                    format!("{}\n\n{}", c.sha, c.commit.message),
                    ".txt",
                )
            }),
            PrTab::Files => (!self.files_lines.is_empty())
                .then_some((self.files_lines.join("\n"), ".txt")),
        }
    }

    fn thread_detail_plain(it: &ThreadItem) -> String {
        match it {
            ThreadItem::Issue { body, created, .. } => {
                format!("{created}\n\n{body}")
            }
            ThreadItem::Review {
                body,
                path,
                line,
                diff_hunk,
                created,
                ..
            } => {
                format!(
                    "{created}\n`{path}` L{line:?}\n\n{body}\n\n```diff\n{}\n```",
                    diff_hunk.chars().take(12000).collect::<String>()
                )
            }
        }
    }
}

fn reaction_kind_label(c: &ReactionContent) -> &'static str {
    use ReactionContent::*;
    match c {
        PlusOne => "👍",
        MinusOne => "👎",
        Laugh => "😄",
        Confused => "😕",
        Heart => "❤",
        Hooray => "🎉",
        Rocket => "🚀",
        Eyes => "👀",
        _ => "·",
    }
}

fn reaction_options() -> Vec<ReactionContent> {
    use ReactionContent::*;
    vec![
        PlusOne,
        MinusOne,
        Laugh,
        Confused,
        Heart,
        Hooray,
        Rocket,
        Eyes,
    ]
}

struct MouseCaptureGuard;

impl MouseCaptureGuard {
    fn new() -> anyhow::Result<Self> {
        crossterm::execute!(std::io::stdout(), crossterm::event::EnableMouseCapture)
            .context("enable mouse capture")?;
        Ok(Self)
    }
}

impl Drop for MouseCaptureGuard {
    fn drop(&mut self) {
        let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture);
    }
}
