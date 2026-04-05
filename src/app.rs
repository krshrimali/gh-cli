use crate::diff_nvim;
use crate::diff_pick;
use crate::editor;
use crate::git;
use crate::markdown_render;
use crate::ui;
use crate::github;
use std::collections::HashMap;
use anyhow::{bail, Context};
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;
use ratatui::widgets::ListState;
use std::cell::Cell;
use octocrab::models::issues::Issue;
use octocrab::models::pulls::PullRequest;
use octocrab::models::repos::RepoCommit;
use octocrab::models::CommentId;
use octocrab::models::reactions::ReactionContent;
use octocrab::models::pulls::ReviewAction;
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

/// Context-sensitive `?` help (per screen / tab / composer).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum HelpContext {
    PrList,
    PrDetailInfo,
    PrDetailThread,
    PrDetailCommits,
    PrDetailFiles,
    PrDetailDiff,
    PrDetailReviews,
    PrDetailReviewsComposer,
}

/// One row in the Reviews tab (submitted reviews list).
#[derive(Clone)]
pub struct CachedReview {
    pub id: u64,
    pub who: String,
    pub state: String,
    pub summary: String,
    pub body: String,
    pub html_url: String,
}

/// Focused pane in the Reviews tab **composer** (split layout, not a popup).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ReviewsComposePane {
    Files,
    Diff,
    Actions,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ReviewsComposerSubphase {
    Normal,
    ConfirmDiscard,
}

/// Draft text for a single pending-review inline comment (docked under the diff, GitHub-style).
#[derive(Clone, Debug)]
pub struct InlineCommentDraft {
    pub path: String,
    /// End line for GitHub `line` (inclusive).
    pub line: u32,
    pub side: String,
    /// Multi-line: GitHub `start_line` (inclusive); omit for single-line comments.
    pub start_line: Option<u32>,
    pub start_side: Option<String>,
    pub chars: Vec<char>,
    pub cursor: usize,
}

/// In-tab pending-review UI: files | diff | finish (GitHub “start review” flow).
#[derive(Clone)]
pub struct ReviewsComposer {
    pub focus: ReviewsComposePane,
    pub subphase: ReviewsComposerSubphase,
    pub file_cursor: usize,
    pub path: String,
    pub diff_lines: Vec<diff_pick::DiffDisplayLine>,
    pub line_cursor: usize,
    pub pending_review_id: u64,
    pub commit_sha: String,
    pub session_comments: Vec<String>,
    /// 0 approve, 1 request changes, 2 comment, 3 discard
    pub submit_cursor: usize,
    /// When set, keyboard focuses this composer (Esc / Ctrl+Enter); diff list is read-only until closed.
    pub comment_draft: Option<InlineCommentDraft>,
    /// `[` / `]` on a commentable row: multi-line review comment (same side).
    pub range_start: Option<(u32, String)>,
    pub range_end: Option<(u32, String)>,
}

impl ReviewsComposer {
    /// `(end_line, side, start_line?, start_side?)` for GitHub inline comment API.
    fn inline_comment_target(&self) -> Result<(u32, String, Option<u32>, Option<String>), &'static str> {
        if let (Some((a, sa)), Some((b, sb))) = (&self.range_start, &self.range_end) {
            if sa != sb {
                return Err("range: [ and ] must be on the same side (LEFT/RIGHT)");
            }
            let lo = (*a).min(*b);
            let hi = (*a).max(*b);
            let side = sa.clone();
            if lo < hi {
                return Ok((hi, side.clone(), Some(lo), Some(side)));
            }
            return Ok((hi, side, None, None));
        }
        let Some(dl) = self.diff_lines.get(self.line_cursor) else {
            return Err("no diff");
        };
        let Some((l, s)) = dl.anchor else {
            return Err("not a commentable line");
        };
        Ok((l, s.to_string(), None, None))
    }
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
    Help(HelpContext),
    /// Full submitted review body (Reviews tab · Enter on a row).
    ReviewDetail {
        title: String,
        body: String,
        url: String,
        scroll: usize,
    },
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
    /// Confirm comment deletion (d key in Thread tab).
    ConfirmDelete { id: CommentId, is_review: bool },
    /// Confirm PR merge (:merge command).
    ConfirmMerge { method: u8 },
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
        pending_review_id: u64,
        commit_sha: String,
        path: String,
        line: u32,
        side: String,
        start_line: Option<u32>,
        start_side: Option<String>,
    },
    /// Finalize a pending review (`SubmitPick` after choosing action).
    SubmitPullReview {
        review_id: u64,
        action: ReviewAction,
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
    pub reviews_cached: Vec<CachedReview>,
    pub review_cursor: usize,
    pub review_scroll: usize,
    /// `z` — hide tab rail for more horizontal space.
    pub hide_tab_rail: bool,
    /// `Z` — hide PR title strip on Thread / Diff / Reviews for taller content.
    pub maximize_pr_content: bool,
    /// Stateful list scroll for Thread and Reviews panes.
    pub thread_list_state: ListState,
    pub review_list_state: ListState,
    pub inline_review_file_state: ListState,
    pub inline_review_line_state: ListState,
    pub inline_review_submit_state: ListState,
    /// Reviews tab: split-pane composer (`a`), None = browse submitted reviews list.
    pub reviews_composer: Option<ReviewsComposer>,
    pub compose_hit_files: Cell<Option<Rect>>,
    pub compose_hit_diff: Cell<Option<Rect>>,
    pub compose_hit_actions: Cell<Option<Rect>>,
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
            reviews_cached: Vec::new(),
            review_cursor: 0,
            review_scroll: 0,
            hide_tab_rail: false,
            maximize_pr_content: false,
            thread_list_state: ListState::default(),
            review_list_state: ListState::default(),
            inline_review_file_state: ListState::default(),
            inline_review_line_state: ListState::default(),
            inline_review_submit_state: ListState::default(),
            reviews_composer: None,
            compose_hit_files: Cell::new(None),
            compose_hit_diff: Cell::new(None),
            compose_hit_actions: Cell::new(None),
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

    /// Which help text to show for `?` / `:help` (per screen and tab).
    pub fn help_context(&self) -> HelpContext {
        match self.screen {
            Screen::PrList => HelpContext::PrList,
            Screen::PrDetail => {
                if self.reviews_composer.is_some() {
                    HelpContext::PrDetailReviewsComposer
                } else {
                    match self.pr_tab {
                        PrTab::Info => HelpContext::PrDetailInfo,
                        PrTab::Thread => HelpContext::PrDetailThread,
                        PrTab::Commits => HelpContext::PrDetailCommits,
                        PrTab::Files => HelpContext::PrDetailFiles,
                        PrTab::Diff => HelpContext::PrDetailDiff,
                        PrTab::Reviews => HelpContext::PrDetailReviews,
                    }
                }
            }
        }
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
        let _kb_guard = KeyboardEnhancementGuard::new();

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
                if self.me.is_none() {
                    self.me = rt.block_on(github::current_login(&self.octo)).ok().flatten();
                }
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
        self.reviews_cached.clear();
        self.review_cursor = 0;
        self.hide_tab_rail = false;
        self.maximize_pr_content = false;
        self.thread_list_state = ListState::default();
        self.review_list_state = ListState::default();
        self.reviews_composer = None;
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
                self.reviews_cached = rs
                    .into_iter()
                    .map(|rev| {
                        let who = rev
                            .user
                            .as_ref()
                            .map(|u| u.login.as_str())
                            .unwrap_or("?")
                            .to_string();
                        let st = rev
                            .state
                            .map(|s| format!("{s:?}"))
                            .unwrap_or_else(|| "?".into());
                        let body = rev.body.unwrap_or_default();
                        let summary = format!(
                            "{who}  [{st}]  {}",
                            body.lines().next().unwrap_or("")
                        );
                        CachedReview {
                            id: rev.id.0,
                            who,
                            state: st,
                            summary,
                            body,
                            html_url: rev.html_url.to_string(),
                        }
                    })
                    .collect();
                self.set_status(format!("{} reviews", self.reviews_cached.len()));
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
            PrTab::Reviews if self.reviews_cached.is_empty() => self.load_reviews(rt)?,
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
                pending_review_id,
                commit_sha,
                path,
                line,
                side,
                start_line,
                start_side,
            } => {
                let _ = self.finish_pending_inline_comment(
                    rt,
                    pr,
                    pending_review_id,
                    &commit_sha,
                    &path,
                    line,
                    &side,
                    &body,
                    start_line,
                    start_side.as_deref(),
                );
            }
            EditorIntent::SubmitPullReview { review_id, action } => {
                let Some(pr_n) = self.pr_number else {
                    return Ok(());
                };
                let r = rt.block_on(github::submit_pull_review(
                    &self.octo,
                    &self.owner,
                    &self.repo,
                    pr_n,
                    review_id,
                    action,
                    &body,
                ));
                match r {
                    Ok(_) => {
                        self.set_status(match action {
                            ReviewAction::Approve => "Review submitted: APPROVED",
                            ReviewAction::RequestChanges => "Review submitted: CHANGES_REQUESTED",
                            ReviewAction::Comment => "Review submitted: COMMENT",
                            _ => "Review submitted",
                        });
                        self.reviews_composer = None;
                        let _ = self.load_thread(rt);
                        let _ = self.load_reviews(rt);
                    }
                    Err(e) => self.set_status(format!("submit review: {e:#}")),
                }
            }
        }
        Ok(())
    }

    pub fn handle_key(&mut self, key: KeyEvent, rt: &Runtime) -> anyhow::Result<AppEffect> {
        if key.kind != crossterm::event::KeyEventKind::Press {
            return Ok(AppEffect::None);
        }

        // Ctrl+C always quits to ensure the terminal is restored cleanly.
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return Ok(AppEffect::Quit);
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
        if matches!(key.code, KeyCode::Esc | KeyCode::Char('q'))
            && matches!(&self.overlay, Overlay::ReviewDetail { .. })
        {
            self.overlay = Overlay::None;
            return Ok(AppEffect::None);
        }

        match &mut self.overlay {
            Overlay::Help(_) => {
                if matches!(key.code, KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?')) {
                    self.overlay = Overlay::None;
                }
                Ok(AppEffect::None)
            }
            Overlay::ReviewDetail {
                scroll,
                url,
                ..
            } => {
                match key.code {
                    KeyCode::Char('j') | KeyCode::Down => {
                        *scroll = scroll.saturating_add(1);
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        *scroll = scroll.saturating_sub(1);
                    }
                    KeyCode::Char('o') if !url.is_empty() => {
                        let _ = open::that(url.as_str());
                    }
                    _ => {}
                }
                if key.modifiers.contains(KeyModifiers::CONTROL) {
                    match key.code {
                        KeyCode::Char('d') => *scroll = scroll.saturating_add(8),
                        KeyCode::Char('u') => *scroll = scroll.saturating_sub(8),
                        _ => {}
                    }
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
            Overlay::ConfirmDelete { id, is_review } => {
                match key.code {
                    KeyCode::Char('y') | KeyCode::Char('Y') => {
                        let cid = *id;
                        let review = *is_review;
                        self.overlay = Overlay::None;
                        if review {
                            let r = rt.block_on(github::delete_review_comment(
                                &self.octo,
                                &self.owner,
                                &self.repo,
                                cid,
                            ));
                            match r {
                                Ok(()) => {
                                    self.set_status("deleted");
                                    let _ = self.load_thread(rt);
                                }
                                Err(e) => self.set_status(format!("delete: {e:#}")),
                            }
                        } else {
                            let r = rt.block_on(github::delete_issue_comment(
                                &self.octo,
                                &self.owner,
                                &self.repo,
                                cid,
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
                    KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                        self.overlay = Overlay::None;
                        self.set_status("delete cancelled");
                    }
                    _ => {}
                }
                Ok(AppEffect::None)
            }
            Overlay::ConfirmMerge { method } => {
                match key.code {
                    KeyCode::Char('y') | KeyCode::Char('Y') => {
                        let m = match *method {
                            1 => MergeMethod::Squash,
                            2 => MergeMethod::Rebase,
                            _ => MergeMethod::Merge,
                        };
                        self.overlay = Overlay::None;
                        if let Some(n) = self.pr_number {
                            let r = rt.block_on(github::merge_pull(
                                &self.octo,
                                &self.owner,
                                &self.repo,
                                n,
                                m,
                            ));
                            match r {
                                Ok(()) => self.set_status("merge requested"),
                                Err(e) => self.set_status(format!("merge: {e:#}")),
                            }
                        }
                    }
                    KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                        self.overlay = Overlay::None;
                        self.set_status("merge cancelled");
                    }
                    _ => {}
                }
                Ok(AppEffect::None)
            }
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
                let method_id: u8 = match parts.get(1).copied().unwrap_or("merge") {
                    "squash" => 1,
                    "rebase" => 2,
                    _ => 0,
                };
                if self.pr_number.is_some() {
                    self.overlay = Overlay::ConfirmMerge { method: method_id };
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
                self.overlay = Overlay::Help(self.help_context());
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
                self.overlay = Overlay::Help(self.help_context());
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
        if let Err(e) = self.ensure_tab_loaded(rt) {
            self.set_status(format!("tab load: {e:#}"));
        }

        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('d') => {
                    self.page_down_current();
                    return Ok(AppEffect::None);
                }
                KeyCode::Char('u') => {
                    self.page_up_current();
                    return Ok(AppEffect::None);
                }
                _ => {}
            }
        }

        if self.pr_tab == PrTab::Reviews && self.reviews_composer.is_some() {
            if let Some(eff) = self.handle_reviews_composer_key(key, rt)? {
                return Ok(eff);
            }
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
                self.overlay = Overlay::Help(self.help_context());
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
                            let (cid, is_review) = match it {
                                ThreadItem::Issue { id, .. } => (*id, false),
                                ThreadItem::Review { id, .. } => (*id, true),
                            };
                            self.overlay = Overlay::ConfirmDelete { id: cid, is_review };
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
            KeyCode::Enter => {
                if self.pr_tab == PrTab::Reviews && self.reviews_composer.is_none() {
                    if let Some(r) = self.reviews_cached.get(self.review_cursor).cloned() {
                        self.overlay = Overlay::ReviewDetail {
                            title: format!("{} · [{}] · review #{}", r.who, r.state, r.id),
                            body: r.body,
                            url: r.html_url,
                            scroll: 0,
                        };
                    }
                }
            }
            KeyCode::Char('z') => {
                self.hide_tab_rail = !self.hide_tab_rail;
                self.set_status(if self.hide_tab_rail {
                    "z: tab rail hidden (more width) — z again to show"
                } else {
                    "z: tab rail visible"
                });
            }
            KeyCode::Char('Z') => {
                self.maximize_pr_content = !self.maximize_pr_content;
                self.set_status(if self.maximize_pr_content {
                    "Z: taller Thread/Diff/Reviews body — Z again to restore title strip"
                } else {
                    "Z: normal PR header"
                });
            }
            KeyCode::Char(c) if matches!(c, '1'..='6') => {
                if let Some(d) = c.to_digit(10) {
                    if let Some(tab) = PrTab::from_digit(d as u8) {
                        if tab != PrTab::Reviews {
                            self.reviews_composer = None;
                        }
                        self.pr_tab = tab;
                        self.tab_scroll = 0;
                        if let Err(e) = self.ensure_tab_loaded(rt) {
                            self.set_status(format!("tab load: {e:#}"));
                        }
                    }
                }
            }
            _ => {
                self.vim_g_pending = false;
            }
        }
        Ok(AppEffect::None)
    }

    /// `Some` = key consumed (return from `handle_pr_detail`). `None` = fall through to normal PR keys.
    fn handle_reviews_composer_key(
        &mut self,
        key: KeyEvent,
        rt: &Runtime,
    ) -> anyhow::Result<Option<AppEffect>> {
        if self.reviews_composer.is_none() {
            return Ok(None);
        }

        if self.reviews_composer.as_ref().unwrap().subphase
            == ReviewsComposerSubphase::ConfirmDiscard
        {
            match key.code {
                KeyCode::Esc | KeyCode::Char('n') => {
                    self.reviews_composer.as_mut().unwrap().subphase =
                        ReviewsComposerSubphase::Normal;
                }
                KeyCode::Char('y') => {
                    let Some(n) = self.pr_number else {
                        return Ok(Some(AppEffect::None));
                    };
                    let rid = self.reviews_composer.as_ref().unwrap().pending_review_id;
                    let r = rt.block_on(github::delete_pending_pull_review(
                        &self.octo,
                        &self.owner,
                        &self.repo,
                        n,
                        rid,
                    ));
                    match r {
                        Ok(_) => {
                            self.set_status("pending review discarded on GitHub");
                            self.reviews_composer = None;
                        }
                        Err(e) => self.set_status(format!("discard review: {e:#}")),
                    }
                }
                _ => {}
            }
            return Ok(Some(AppEffect::None));
        }

        if self
            .reviews_composer
            .as_ref()
            .is_some_and(|c| c.comment_draft.is_some())
        {
            return self.handle_reviews_comment_draft_key(key, rt);
        }

        match key.code {
            KeyCode::Esc => {
                self.reviews_composer = None;
                self.set_status(
                    "left composer — pending review still on GitHub (submit or discard there / reopen a)",
                );
                return Ok(Some(AppEffect::None));
            }
            KeyCode::Tab => {
                let c = self.reviews_composer.as_mut().unwrap();
                c.focus = match c.focus {
                    ReviewsComposePane::Files => ReviewsComposePane::Diff,
                    ReviewsComposePane::Diff => ReviewsComposePane::Actions,
                    ReviewsComposePane::Actions => ReviewsComposePane::Files,
                };
                return Ok(Some(AppEffect::None));
            }
            KeyCode::BackTab => {
                let c = self.reviews_composer.as_mut().unwrap();
                c.focus = match c.focus {
                    ReviewsComposePane::Files => ReviewsComposePane::Actions,
                    ReviewsComposePane::Diff => ReviewsComposePane::Files,
                    ReviewsComposePane::Actions => ReviewsComposePane::Diff,
                };
                return Ok(Some(AppEffect::None));
            }
            _ => {}
        }

        let comp = self.reviews_composer.as_mut().unwrap();
        match comp.focus {
            ReviewsComposePane::Files => match key.code {
                KeyCode::Char('j') | KeyCode::Down => {
                    if !self.file_paths.is_empty() {
                        comp.file_cursor =
                            (comp.file_cursor + 1).min(self.file_paths.len() - 1);
                    }
                    Ok(Some(AppEffect::None))
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    comp.file_cursor = comp.file_cursor.saturating_sub(1);
                    Ok(Some(AppEffect::None))
                }
                KeyCode::Enter => {
                    if self.file_paths.is_empty() {
                        self.set_status("no files on this PR");
                    } else if let Some(p) = self.file_paths.get(comp.file_cursor).cloned() {
                        match diff_pick::extract_file_patch(&self.diff_text, &p) {
                            None => self.set_status(format!("no diff chunk for `{p}`")),
                            Some(chunk) => {
                                if chunk.contains("Binary files ") && chunk.contains(" differ") {
                                    self.set_status("binary file — pick another");
                                } else {
                                    let lines = diff_pick::parse_patch_lines(chunk);
                                    if diff_pick::first_anchor_index(&lines).is_none() {
                                        self.set_status("no commentable lines in this chunk");
                                    } else {
                                        comp.path = p;
                                        comp.diff_lines = lines;
                                        comp.line_cursor = diff_pick::first_anchor_index(
                                            &comp.diff_lines,
                                        )
                                        .unwrap_or(0);
                                        comp.focus = ReviewsComposePane::Diff;
                                        self.inline_review_line_state = ListState::default();
                                        self.set_status(
                                            "Diff: Enter on a line with + opens Write box · n/p · Tab",
                                        );
                                    }
                                }
                            }
                        }
                    }
                    Ok(Some(AppEffect::None))
                }
                _ => Ok(None),
            },
            ReviewsComposePane::Diff => match key.code {
                KeyCode::Char('j') | KeyCode::Down => {
                    if !comp.diff_lines.is_empty() {
                        comp.line_cursor =
                            (comp.line_cursor + 1).min(comp.diff_lines.len() - 1);
                    }
                    Ok(Some(AppEffect::None))
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    comp.line_cursor = comp.line_cursor.saturating_sub(1);
                    Ok(Some(AppEffect::None))
                }
                KeyCode::Char('n') => {
                    if !comp.diff_lines.is_empty() {
                        comp.line_cursor = diff_pick::step_anchor(
                            comp.line_cursor,
                            &comp.diff_lines,
                            true,
                        );
                    }
                    Ok(Some(AppEffect::None))
                }
                KeyCode::Char('p') => {
                    if !comp.diff_lines.is_empty() {
                        comp.line_cursor = diff_pick::step_anchor(
                            comp.line_cursor,
                            &comp.diff_lines,
                            false,
                        );
                    }
                    Ok(Some(AppEffect::None))
                }
                KeyCode::Char('[') => {
                    if comp.path.is_empty() {
                        self.set_status("load a file diff first");
                        return Ok(Some(AppEffect::None));
                    }
                    if let Some(dl) = comp.diff_lines.get(comp.line_cursor) {
                        if let Some((ln, side)) = dl.anchor {
                            comp.range_start = Some((ln, side.to_string()));
                            self.set_status(format!(
                                "range start: L{ln} ({side}) — move to end row, press ]"
                            ));
                            return Ok(Some(AppEffect::None));
                        }
                    }
                    self.set_status("[ on a commentable line (n/p) — start of multi-line comment");
                    Ok(Some(AppEffect::None))
                }
                KeyCode::Char(']') => {
                    if comp.path.is_empty() {
                        self.set_status("load a file diff first");
                        return Ok(Some(AppEffect::None));
                    }
                    if let Some(dl) = comp.diff_lines.get(comp.line_cursor) {
                        if let Some((ln, side)) = dl.anchor {
                            comp.range_end = Some((ln, side.to_string()));
                            self.set_status(format!(
                                "range end: L{ln} ({side}) — Enter opens write box (same side as [ )"
                            ));
                            return Ok(Some(AppEffect::None));
                        }
                    }
                    self.set_status("] on a commentable line — end of multi-line range");
                    Ok(Some(AppEffect::None))
                }
                KeyCode::Char('r') => {
                    comp.range_start = None;
                    comp.range_end = None;
                    self.set_status("cleared [ ] multi-line range");
                    Ok(Some(AppEffect::None))
                }
                KeyCode::Enter => {
                    if comp.path.is_empty() {
                        self.set_status("Files pane: Enter loads a diff into this pane");
                        return Ok(Some(AppEffect::None));
                    }
                    match comp.inline_comment_target() {
                        Err(msg) => {
                            self.set_status(msg.to_string());
                            Ok(Some(AppEffect::None))
                        }
                        Ok((line, side, start_line, start_side)) => {
                            let path = comp.path.clone();
                            comp.comment_draft = Some(InlineCommentDraft {
                                path,
                                line,
                                side: side.clone(),
                                start_line,
                                start_side,
                                chars: Vec::new(),
                                cursor: 0,
                            });
                            self.set_status(
                                "Write · Ctrl+Enter / Alt+Enter / Ctrl+s / F2 post · Esc · Ctrl+e editor",
                            );
                            Ok(Some(AppEffect::None))
                        }
                    }
                }
                KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    let Some(n) = self.pr_number else {
                        return Ok(Some(AppEffect::None));
                    };
                    if comp.path.is_empty() {
                        self.set_status("Load a file diff first (Files pane · Enter)");
                        return Ok(Some(AppEffect::None));
                    }
                    let pend = comp.pending_review_id;
                    let sha = comp.commit_sha.clone();
                    let path = comp.path.clone();
                    match comp.inline_comment_target() {
                        Err(msg) => {
                            self.set_status(msg.to_string());
                            Ok(Some(AppEffect::None))
                        }
                        Ok((line, side, start_line, start_side)) => {
                            let initial = if let Some(lo) = start_line {
                                format!(
                                    "## `{path}` multi-line L{lo}–L{line} ({side})\n\n\
Comment (markdown). Optional suggestion:\n\n\
```suggestion\n\n```\n",
                                )
                            } else {
                                format!(
                                    "## `{path}` line {line} ({side})\n\n\
Comment (markdown). Optional suggestion:\n\n\
```suggestion\n\n```\n",
                                )
                            };
                            return Ok(Some(AppEffect::OpenEditor {
                                initial,
                                intent: EditorIntent::InlineReviewComment {
                                    pr: n,
                                    pending_review_id: pend,
                                    commit_sha: sha,
                                    path,
                                    line,
                                    side,
                                    start_line,
                                    start_side,
                                },
                            }));
                        }
                    }
                }
                _ => Ok(None),
            },
            ReviewsComposePane::Actions => match key.code {
                KeyCode::Char('j') | KeyCode::Down => {
                    comp.submit_cursor = (comp.submit_cursor + 1).min(3);
                    Ok(Some(AppEffect::None))
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    comp.submit_cursor = comp.submit_cursor.saturating_sub(1);
                    Ok(Some(AppEffect::None))
                }
                KeyCode::Enter => {
                    let Some(n) = self.pr_number else {
                        return Ok(Some(AppEffect::None));
                    };
                    let rid = comp.pending_review_id;
                    match comp.submit_cursor {
                        0 => {
                            let r = rt.block_on(github::submit_pull_review(
                                &self.octo,
                                &self.owner,
                                &self.repo,
                                n,
                                rid,
                                ReviewAction::Approve,
                                "",
                            ));
                            match r {
                                Ok(_) => {
                                    self.set_status("Review submitted: APPROVED");
                                    self.reviews_composer = None;
                                    let _ = self.load_thread(rt);
                                    let _ = self.load_reviews(rt);
                                }
                                Err(e) => self.set_status(format!("submit: {e:#}")),
                            }
                            Ok(Some(AppEffect::None))
                        }
                        1 => Ok(Some(AppEffect::OpenEditor {
                            initial: "## Request changes\n\n".to_string(),
                            intent: EditorIntent::SubmitPullReview {
                                review_id: rid,
                                action: ReviewAction::RequestChanges,
                            },
                        })),
                        2 => Ok(Some(AppEffect::OpenEditor {
                            initial: "## Comment\n\n".to_string(),
                            intent: EditorIntent::SubmitPullReview {
                                review_id: rid,
                                action: ReviewAction::Comment,
                            },
                        })),
                        3 => {
                            comp.subphase = ReviewsComposerSubphase::ConfirmDiscard;
                            Ok(Some(AppEffect::None))
                        }
                        _ => Ok(Some(AppEffect::None)),
                    }
                }
                _ => Ok(None),
            },
        }
    }

    fn start_inline_review_wizard(&mut self, rt: &Runtime) -> anyhow::Result<()> {
        if self.reviews_composer.is_some() {
            self.set_status("already in review composer — Esc to close");
            return Ok(());
        }
        let Some(n) = self.pr_number else {
            return Ok(());
        };
        let Some(pr) = self.current_pr.as_ref() else {
            return Ok(());
        };
        let commit_sha = pr.head.sha.clone();
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
        if self.me.is_none() {
            self.me = rt.block_on(github::current_login(&self.octo)).ok().flatten();
        }
        self.loading = true;
        let oct = self.octo.clone();
        let owner = self.owner.clone();
        let repo = self.repo.clone();
        let me = self.me.clone();
        let pending = rt.block_on(github::ensure_pending_pull_review(
            &oct,
            &owner,
            &repo,
            n,
            &commit_sha,
            me.as_deref(),
        ));
        self.loading = false;
        let pending_review_id = match pending {
            Ok(id) => id,
            Err(e) => {
                self.set_status(format!("could not start pending review: {e:#}"));
                return Ok(());
            }
        };
        self.inline_review_file_state = ListState::default();
        self.inline_review_line_state = ListState::default();
        self.inline_review_submit_state = ListState::default();
        self.reviews_composer = Some(ReviewsComposer {
            focus: ReviewsComposePane::Files,
            subphase: ReviewsComposerSubphase::Normal,
            file_cursor: 0,
            path: String::new(),
            diff_lines: Vec::new(),
            line_cursor: 0,
            pending_review_id,
            commit_sha,
            session_comments: Vec::new(),
            submit_cursor: 0,
            comment_draft: None,
            range_start: None,
            range_end: None,
        });
        self.pr_tab = PrTab::Reviews;
        self.set_status(format!(
            "composer: Tab switches panes (Files → Diff → Finish) · pending #{pending_review_id}"
        ));
        Ok(())
    }

    fn handle_reviews_compose_mouse(&mut self, m: MouseEvent) {
        fn inside(col: u16, row: u16, r: Rect) -> bool {
            col >= r.x
                && col < r.x.saturating_add(r.width)
                && row >= r.y
                && row < r.y.saturating_add(r.height)
        }
        let Some(comp) = self.reviews_composer.as_mut() else {
            return;
        };
        if comp.comment_draft.is_some() {
            return;
        }
        if comp.subphase == ReviewsComposerSubphase::ConfirmDiscard {
            return;
        }
        let col = m.column;
        let row = m.row;
        if let Some(r) = self.compose_hit_files.get() {
            if inside(col, row, r) {
                comp.focus = ReviewsComposePane::Files;
                let dy = (row - r.y) as usize;
                if dy < self.file_paths.len() {
                    comp.file_cursor = dy;
                }
                return;
            }
        }
        if let Some(r) = self.compose_hit_diff.get() {
            if inside(col, row, r) {
                comp.focus = ReviewsComposePane::Diff;
                let dy = (row - r.y) as usize;
                if dy < comp.diff_lines.len() {
                    comp.line_cursor = dy;
                }
                return;
            }
        }
        if let Some(r) = self.compose_hit_actions.get() {
            if inside(col, row, r) {
                comp.focus = ReviewsComposePane::Actions;
                let dy = (row - r.y) as usize;
                if dy <= 3 {
                    comp.submit_cursor = dy;
                }
            }
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
                if !self.reviews_cached.is_empty() {
                    self.review_cursor =
                        (self.review_cursor + 1).min(self.reviews_cached.len() - 1);
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
            Overlay::None => {}
            _ => return Ok(()),
        }
        if self.screen == Screen::PrDetail
            && self.pr_tab == PrTab::Reviews
            && self.reviews_composer.is_some()
        {
            self.handle_reviews_compose_mouse(m);
            return Ok(());
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
            PrTab::Reviews => (!self.reviews_cached.is_empty()).then_some((
                self.reviews_cached
                    .iter()
                    .map(|r| r.summary.as_str())
                    .collect::<Vec<_>>()
                    .join("\n"),
                ".txt",
            )),
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

    /// Refreshes PR head + unified diff so `commit_id` matches the patch used for line anchors.
    fn sync_review_composer_to_pr_head(&mut self, rt: &Runtime) -> anyhow::Result<()> {
        let Some(n) = self.pr_number else {
            return Ok(());
        };
        let Some(comp) = self.reviews_composer.as_mut() else {
            return Ok(());
        };

        let pull = rt.block_on(github::get_pull(
            &self.octo,
            &self.owner,
            &self.repo,
            n,
        ))?;
        let new_sha = pull.head.sha.clone();
        self.current_pr = Some(pull);

        let sha_changed = new_sha != comp.commit_sha;
        if !sha_changed && !self.diff_text.is_empty() {
            return Ok(());
        }

        comp.commit_sha = new_sha;

        let mut diff_s = rt.block_on(github::get_pr_diff(
            &self.octo,
            &self.owner,
            &self.repo,
            n,
        ))?;
        const MAX: usize = 256 * 1024;
        if diff_s.len() > MAX {
            diff_s.truncate(MAX);
            diff_s.push_str("\n\n… truncated (diff too large) …");
        }
        self.diff_text = diff_s;

        let path = comp.path.clone();
        if path.is_empty() {
            return Ok(());
        }

        let anchor_before = comp
            .diff_lines
            .get(comp.line_cursor)
            .and_then(|l| l.anchor)
            .map(|(ln, s)| (ln, s.to_string()));

        match diff_pick::extract_file_patch(&self.diff_text, &path) {
            Some(chunk) if !(chunk.contains("Binary files ") && chunk.contains(" differ")) => {
                let lines = diff_pick::parse_patch_lines(chunk);
                comp.diff_lines = lines;
                comp.line_cursor = if let Some((ln, ref side)) = anchor_before {
                    let want: &'static str = match side.as_str() {
                        "LEFT" => "LEFT",
                        _ => "RIGHT",
                    };
                    comp.diff_lines
                        .iter()
                        .position(|l| l.anchor == Some((ln, want)))
                        .or_else(|| diff_pick::first_anchor_index(&comp.diff_lines))
                        .unwrap_or(0)
                } else {
                    diff_pick::first_anchor_index(&comp.diff_lines).unwrap_or(0)
                };
            }
            _ => {
                comp.comment_draft = None;
                comp.range_start = None;
                comp.range_end = None;
                comp.diff_lines.clear();
                comp.path.clear();
                bail!(
                    "PR changed on GitHub — that file is gone from the diff. Pick it again in Files (diff refreshed)"
                );
            }
        }

        Ok(())
    }

    /// Post one inline comment via `POST …/pulls/{pr}/comments` only (does **not** create a review).
    fn finish_pending_inline_comment(
        &mut self,
        rt: &Runtime,
        pr: u64,
        pending_review_id: u64,
        commit_sha: &str,
        path: &str,
        line: u32,
        side: &str,
        body: &str,
        start_line: Option<u32>,
        start_side: Option<&str>,
    ) -> anyhow::Result<()> {
        if self.reviews_composer.is_some() {
            self.sync_review_composer_to_pr_head(rt)?;
            if !diff_pick::patch_has_anchor(&self.diff_text, path, line, side) {
                bail!(
                    "line {line} ({side}) is not in the current diff for `{path}` — pick the line again (branch may have moved)"
                );
            }
        }
        let commit_sha = self
            .reviews_composer
            .as_ref()
            .map(|c| c.commit_sha.as_str())
            .unwrap_or(commit_sha);

        let r = rt.block_on(github::create_pull_review_inline_comment(
            &self.octo,
            &self.owner,
            &self.repo,
            pr,
            commit_sha,
            path,
            line,
            side,
            body,
            start_line,
            start_side,
        ));
        match r {
            Ok(_) => {
                let peek = PrListEntry::ellipsize(body.lines().next().unwrap_or("(empty)"), 72);
                let loc = if let Some(sl) = start_line {
                    format!("`{path}` L{sl}–L{line}")
                } else {
                    format!("`{path}` L{line}")
                };
                let summary = format!("{loc}: {peek}");
                if let Some(comp) = &mut self.reviews_composer {
                    comp.session_comments.push(summary);
                    comp.range_start = None;
                    comp.range_end = None;
                }
                self.set_status(format!(
                    "added to pending review #{pending_review_id} — pick another line or ▸ Finish"
                ));
                let _ = self.load_thread(rt);
                Ok(())
            }
            Err(e) => {
                let report = github::format_inline_comment_octocrab_err(&e);
                eprintln!("gh-pr-cli: inline comment failed\n{report}\n(full: {e:#})\n");
                self.set_status(format!("inline comment: {report}"));
                Err(e.into())
            }
        }
    }

    /// Submit the current inline comment draft to the pending review. Shared by all submit keybindings.
    fn submit_comment_draft(&mut self, rt: &Runtime) -> anyhow::Result<Option<AppEffect>> {
        let pr_n = self.pr_number;
        let (body, path, line, side, sl, ss, pend, sha) = {
            let c = self.reviews_composer.as_mut().unwrap();
            let d = c.comment_draft.as_ref().unwrap();
            (
                d.chars.iter().collect::<String>(),
                d.path.clone(),
                d.line,
                d.side.clone(),
                d.start_line,
                d.start_side.clone(),
                c.pending_review_id,
                c.commit_sha.clone(),
            )
        };
        let trimmed = body.trim();
        if trimmed.is_empty() {
            self.set_status("empty comment — add text or Esc");
            return Ok(Some(AppEffect::None));
        }
        let Some(pr) = pr_n else {
            return Ok(Some(AppEffect::None));
        };
        let ok = self
            .finish_pending_inline_comment(
                rt,
                pr,
                pend,
                &sha,
                &path,
                line,
                &side,
                trimmed,
                sl,
                ss.as_deref(),
            )
            .is_ok();
        if ok {
            if let Some(c) = self.reviews_composer.as_mut() {
                c.comment_draft = None;
            }
        }
        Ok(Some(AppEffect::None))
    }

    fn handle_reviews_comment_draft_key(
        &mut self,
        key: KeyEvent,
        rt: &Runtime,
    ) -> anyhow::Result<Option<AppEffect>> {
        if key.kind != KeyEventKind::Press {
            return Ok(Some(AppEffect::None));
        }
        if !self
            .reviews_composer
            .as_ref()
            .is_some_and(|c| c.comment_draft.is_some())
        {
            return Ok(Some(AppEffect::None));
        }

        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);

        match key.code {
            KeyCode::Esc => {
                if let Some(c) = self.reviews_composer.as_mut() {
                    c.comment_draft = None;
                }
                self.set_status("comment draft closed");
                Ok(Some(AppEffect::None))
            }
            // Submit: Ctrl+Enter, Alt+Enter, Ctrl+S, F2
            KeyCode::Enter if ctrl || alt => self.submit_comment_draft(rt),
            KeyCode::Char('s') if ctrl => self.submit_comment_draft(rt),
            KeyCode::F(2) => self.submit_comment_draft(rt),
            KeyCode::Char('e') if ctrl => {
                let Some(pr) = self.pr_number else {
                    return Ok(Some(AppEffect::None));
                };
                let Some(taken) = self
                    .reviews_composer
                    .as_mut()
                    .and_then(|c| c.comment_draft.take())
                else {
                    return Ok(Some(AppEffect::None));
                };
                let path = taken.path;
                let line = taken.line;
                let side = taken.side;
                let start_line = taken.start_line;
                let start_side = taken.start_side;
                let (pend, sha) = {
                    let c = self.reviews_composer.as_ref().unwrap();
                    (c.pending_review_id, c.commit_sha.clone())
                };
                let initial: String = taken.chars.iter().collect();
                let initial = if !initial.trim().is_empty() {
                    initial
                } else if let Some(lo) = start_line {
                    format!("## `{path}` multi-line L{lo}–L{line} ({side})\n\n")
                } else {
                    format!("## `{path}` line {line} ({side})\n\n")
                };
                Ok(Some(AppEffect::OpenEditor {
                    initial,
                    intent: EditorIntent::InlineReviewComment {
                        pr,
                        pending_review_id: pend,
                        commit_sha: sha,
                        path,
                        line,
                        side,
                        start_line,
                        start_side,
                    },
                }))
            }
            KeyCode::Char(c) if !ctrl && !alt => {
                let Some(d) = self
                    .reviews_composer
                    .as_mut()
                    .and_then(|c| c.comment_draft.as_mut())
                else {
                    return Ok(Some(AppEffect::None));
                };
                let i = d.cursor.min(d.chars.len());
                d.chars.insert(i, c);
                d.cursor = i + 1;
                Ok(Some(AppEffect::None))
            }
            KeyCode::Backspace => {
                let Some(d) = self
                    .reviews_composer
                    .as_mut()
                    .and_then(|c| c.comment_draft.as_mut())
                else {
                    return Ok(Some(AppEffect::None));
                };
                if d.cursor > 0 {
                    d.cursor -= 1;
                    d.chars.remove(d.cursor);
                }
                Ok(Some(AppEffect::None))
            }
            KeyCode::Delete => {
                let Some(d) = self
                    .reviews_composer
                    .as_mut()
                    .and_then(|c| c.comment_draft.as_mut())
                else {
                    return Ok(Some(AppEffect::None));
                };
                if d.cursor < d.chars.len() {
                    d.chars.remove(d.cursor);
                }
                Ok(Some(AppEffect::None))
            }
            KeyCode::Left => {
                let Some(d) = self
                    .reviews_composer
                    .as_mut()
                    .and_then(|c| c.comment_draft.as_mut())
                else {
                    return Ok(Some(AppEffect::None));
                };
                d.cursor = d.cursor.saturating_sub(1);
                Ok(Some(AppEffect::None))
            }
            KeyCode::Right => {
                let Some(d) = self
                    .reviews_composer
                    .as_mut()
                    .and_then(|c| c.comment_draft.as_mut())
                else {
                    return Ok(Some(AppEffect::None));
                };
                if d.cursor < d.chars.len() {
                    d.cursor += 1;
                }
                Ok(Some(AppEffect::None))
            }
            KeyCode::Home => {
                let Some(d) = self
                    .reviews_composer
                    .as_mut()
                    .and_then(|c| c.comment_draft.as_mut())
                else {
                    return Ok(Some(AppEffect::None));
                };
                d.cursor = draft_line_start(&d.chars, d.cursor);
                Ok(Some(AppEffect::None))
            }
            KeyCode::End => {
                let Some(d) = self
                    .reviews_composer
                    .as_mut()
                    .and_then(|c| c.comment_draft.as_mut())
                else {
                    return Ok(Some(AppEffect::None));
                };
                d.cursor = draft_line_end(&d.chars, d.cursor);
                Ok(Some(AppEffect::None))
            }
            KeyCode::Up => {
                let Some(d) = self
                    .reviews_composer
                    .as_mut()
                    .and_then(|c| c.comment_draft.as_mut())
                else {
                    return Ok(Some(AppEffect::None));
                };
                d.cursor = draft_move_up(&d.chars, d.cursor);
                Ok(Some(AppEffect::None))
            }
            KeyCode::Down => {
                let Some(d) = self
                    .reviews_composer
                    .as_mut()
                    .and_then(|c| c.comment_draft.as_mut())
                else {
                    return Ok(Some(AppEffect::None));
                };
                d.cursor = draft_move_down(&d.chars, d.cursor);
                Ok(Some(AppEffect::None))
            }
            KeyCode::Enter => {
                let Some(d) = self
                    .reviews_composer
                    .as_mut()
                    .and_then(|c| c.comment_draft.as_mut())
                else {
                    return Ok(Some(AppEffect::None));
                };
                let i = d.cursor.min(d.chars.len());
                d.chars.insert(i, '\n');
                d.cursor = i + 1;
                Ok(Some(AppEffect::None))
            }
            KeyCode::Tab => Ok(Some(AppEffect::None)),
            _ => Ok(Some(AppEffect::None)),
        }
    }
}

fn draft_line_start(chars: &[char], pos: usize) -> usize {
    let pos = pos.min(chars.len());
    chars[..pos]
        .iter()
        .rposition(|c| *c == '\n')
        .map(|i| i + 1)
        .unwrap_or(0)
}

fn draft_line_end(chars: &[char], pos: usize) -> usize {
    let pos = pos.min(chars.len());
    chars[pos..]
        .iter()
        .position(|c| *c == '\n')
        .map(|i| pos + i)
        .unwrap_or(chars.len())
}

fn draft_move_up(chars: &[char], cur: usize) -> usize {
    let ls = draft_line_start(chars, cur);
    if ls == 0 {
        return 0;
    }
    let prev_line_end = ls.saturating_sub(1);
    let prev_start = draft_line_start(chars, prev_line_end);
    let col = cur.saturating_sub(ls);
    let prev_len = prev_line_end.saturating_sub(prev_start);
    let target_col = col.min(prev_len);
    prev_start + target_col
}

fn draft_move_down(chars: &[char], cur: usize) -> usize {
    let le = draft_line_end(chars, cur);
    if le >= chars.len() {
        return cur;
    }
    let next_start = le + 1;
    let next_end = draft_line_end(chars, next_start);
    let ls = draft_line_start(chars, cur);
    let col = cur.saturating_sub(ls);
    let next_len = next_end.saturating_sub(next_start);
    let target_col = col.min(next_len);
    next_start + target_col
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

/// Enables the Kitty keyboard protocol so that Ctrl+Enter is distinguishable from Enter.
/// Gracefully does nothing on terminals that don't support it.
struct KeyboardEnhancementGuard {
    enabled: bool,
}

impl KeyboardEnhancementGuard {
    fn new() -> Self {
        let ok = crossterm::execute!(
            std::io::stdout(),
            crossterm::event::PushKeyboardEnhancementFlags(
                crossterm::event::KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
            )
        );
        Self { enabled: ok.is_ok() }
    }
}

impl Drop for KeyboardEnhancementGuard {
    fn drop(&mut self) {
        if self.enabled {
            let _ = crossterm::execute!(
                std::io::stdout(),
                crossterm::event::PopKeyboardEnhancementFlags
            );
        }
    }
}
