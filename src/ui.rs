//! Layout and styling (Catppuccin-inspired palette on dark backgrounds).

use crate::app::{
    App, FilterPanelPhase, InlineCommentDraft, Overlay, PrListEntry, PrTab, ReviewsComposePane,
    ReviewsComposerSubphase, Screen, ThreadItem,
};
use crate::diff_pick::{DiffDisplayLine, DiffLineKind};
use crate::github;
use crate::markdown_render;
use octocrab::models::CommentId;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};
use ratatui::Frame;
use std::collections::HashMap;

const BG: Color = Color::Rgb(30, 30, 46);
const SURFACE: Color = Color::Rgb(49, 50, 68);
const TEXT: Color = Color::Rgb(205, 214, 244);
const SUB: Color = Color::Rgb(166, 173, 200);
const ACCENT: Color = Color::Rgb(137, 180, 250);
const GREEN: Color = Color::Rgb(166, 227, 161);
const PEACH: Color = Color::Rgb(250, 179, 135);
const MAUVE: Color = Color::Rgb(203, 166, 247);
const RED: Color = Color::Rgb(243, 139, 168);
/// GitHub-like diff row tint (dark Catppuccin).
const DIFF_DEL_BG: Color = Color::Rgb(52, 36, 42);
const DIFF_ADD_BG: Color = Color::Rgb(36, 48, 42);

pub fn draw(f: &mut Frame<'_>, app: &mut App) {
    app.pr_list_hit_rect.set(None);
    app.compose_hit_files.set(None);
    app.compose_hit_diff.set(None);
    app.compose_hit_actions.set(None);
    let full = f.area();
    f.render_widget(
        Block::default().style(Style::default().bg(BG)),
        full,
    );

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(if app.loading { 1 } else { 0 }),
            Constraint::Min(0),
            Constraint::Length(2),
        ])
        .split(full);

    draw_header(f, app, chunks[0]);

    if app.loading {
        let line = Line::from(vec![Span::styled(
            "  … loading …",
            Style::default().fg(MAUVE).add_modifier(Modifier::ITALIC),
        )]);
        f.render_widget(Paragraph::new(line), chunks[1]);
    }

    let body = chunks[2];
    match app.screen {
        Screen::PrList => draw_pr_list(f, app, body),
        Screen::PrDetail => draw_pr_detail(f, app, body),
    }

    draw_status(f, app, chunks[3]);

    draw_overlay(f, app, full);
}

fn draw_header(f: &mut Frame<'_>, app: &mut App, area: Rect) {
    let who = app
        .me
        .as_deref()
        .map(|s| format!(" @{s}"))
        .unwrap_or_default();
    let title = format!(" gh-pr-cli  —  {}/{}{} ", app.owner, app.repo, who);
    let block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::default().fg(ACCENT))
        .style(Style::default().bg(SURFACE).fg(TEXT));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let line = Line::from(vec![
        Span::styled("◆ ", Style::default().fg(MAUVE)),
        Span::styled(
            title.trim(),
            Style::default()
                .fg(TEXT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("  vim-style keys  ·  ? help", Style::default().fg(SUB)),
    ]);
    f.render_widget(
        Paragraph::new(line).alignment(Alignment::Left),
        inner,
    );
}

fn draw_status(f: &mut Frame<'_>, app: &mut App, area: Rect) {
    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(SURFACE));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let line = Line::from(Span::styled(
        format!(" {}", app.status),
        Style::default().fg(SUB),
    ));
    f.render_widget(Paragraph::new(line), inner);
}

fn draw_pr_list(f: &mut Frame<'_>, app: &mut App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT))
        .title(Line::from(vec![
            Span::styled(" Pull requests ", Style::default().fg(ACCENT).bold()),
            Span::styled(
                format!("({}) ", app.pr_status.label()),
                Style::default().fg(SUB),
            ),
            Span::styled(
                format!(
                    "{} · pg {} {}",
                    if github::pr_list_uses_search(&app.pr_filters, app.pr_status) {
                        "search"
                    } else {
                        "REST"
                    },
                    if app.pr_list_page == 0 {
                        "—".to_string()
                    } else {
                        format!("{}", app.pr_list_page)
                    },
                    app.pr_list_total_count
                        .map(|n| format!("· ~{n} hits "))
                        .unwrap_or_default(),
                ),
                Style::default().fg(SUB),
            ),
            Span::styled(
                format!(
                    "{} loaded{} ",
                    app.pr_entries.len(),
                    if app.pr_list_has_more {
                        "· m more "
                    } else if app.pr_list_page > 0 {
                        "· end "
                    } else {
                        ""
                    },
                ),
                Style::default().fg(GREEN),
            ),
        ]))
        .style(Style::default().bg(BG));
    let inner = block.inner(area);
    f.render_widget(block, area);
    app.pr_list_hit_rect.set(Some(inner));

    let items: Vec<ListItem> = app
        .pr_entries
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let sel = i == app.pr_cursor;
            let num = e.number();
            let title = e.title();
            let author = e.author_login();
            let badges = e.status_badges();
            let meta = e.meta_summary();
            let line1 = Line::from(vec![
                Span::styled(
                    format!("#{num} "),
                    Style::default().fg(if sel { GREEN } else { SUB }),
                ),
                Span::styled(
                    format!("{badges} "),
                    Style::default().fg(PEACH),
                ),
                Span::styled(
                    format!("@{author}  "),
                    Style::default().fg(MAUVE),
                ),
                Span::styled(meta, Style::default().fg(SUB)),
            ]);
            let title_disp = PrListEntry::ellipsize(title, inner.width.saturating_sub(4) as usize);
            let line2 = Line::from(vec![Span::styled(
                format!("    {title_disp}"),
                Style::default().fg(if sel { TEXT } else { SUB }),
            )]);
            let style = if sel {
                Style::default().bg(SURFACE)
            } else {
                Style::default()
            };
            ListItem::new(Text::from(vec![line1, line2])).style(style)
        })
        .collect();

    let list = List::new(items).highlight_style(
        Style::default()
            .bg(SURFACE)
            .fg(TEXT)
            .add_modifier(Modifier::BOLD),
    );
    f.render_widget(list, inner);
}

fn draw_pr_detail(f: &mut Frame<'_>, app: &mut App, area: Rect) {
    let h_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(18), Constraint::Min(0)])
        .split(area);

    let tabs: Vec<Line> = [
        PrTab::Info,
        PrTab::Thread,
        PrTab::Commits,
        PrTab::Files,
        PrTab::Diff,
        PrTab::Reviews,
    ]
    .iter()
    .map(|t| {
        let active = *t == app.pr_tab;
        Line::from(Span::styled(
            t.label(),
            Style::default().fg(if active { GREEN } else { SUB }).bold(),
        ))
    })
    .collect();

    let tab_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(MAUVE))
        .title(Line::from(Span::styled(" tabs ", Style::default().fg(MAUVE))));
    let tab_inner = tab_block.inner(h_chunks[0]);
    f.render_widget(tab_block, h_chunks[0]);
    f.render_widget(Paragraph::new(tabs), tab_inner);

    let main = h_chunks[1];
    if let Some(pr) = app.current_pr.clone() {
        let title = pr.title.as_deref().unwrap_or("(no title)");
        let head = Line::from(vec![
            Span::styled(
                format!("#{} ", pr.number),
                Style::default().fg(ACCENT).bold(),
            ),
            Span::styled(title, Style::default().fg(TEXT).bold()),
        ]);
        let meta = format!(
            "{} → {}  (+{} −{} files {})",
            pr.head.ref_field,
            pr.base.ref_field,
            pr.additions.unwrap_or(0),
            pr.deletions.unwrap_or(0),
            pr.changed_files.unwrap_or(0),
        );
        let header_h = 4u16;
        let v = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(header_h),
                Constraint::Min(0),
            ])
            .split(main);

        let hb = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(ACCENT));
        let hi = hb.inner(v[0]);
        f.render_widget(hb, v[0]);
        f.render_widget(Paragraph::new(head), hi);
        f.render_widget(
            Paragraph::new(Span::styled(meta, Style::default().fg(SUB))).wrap(Wrap { trim: true }),
            Rect {
                x: hi.x,
                y: hi.y + 1,
                width: hi.width,
                height: hi.height.saturating_sub(1),
            },
        );

        match app.pr_tab {
            PrTab::Info => draw_tab_info(f, app, &pr, v[1]),
            PrTab::Thread => draw_tab_thread(f, app, v[1]),
            PrTab::Commits => draw_tab_commits(f, app, v[1]),
            PrTab::Files => draw_tab_files(f, app, v[1]),
            PrTab::Diff => draw_tab_diff(f, app, v[1]),
            PrTab::Reviews => draw_tab_reviews(f, app, v[1]),
        }
    } else {
        f.render_widget(
            Paragraph::new("No PR loaded").block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(RED)),
            ),
            main,
        );
    }
}

fn draw_tab_info(f: &mut Frame<'_>, app: &mut App, pr: &octocrab::models::pulls::PullRequest, area: Rect) {
    let body = pr.body.as_deref().unwrap_or("_No description provided._");
    let p = Paragraph::new(body)
        .wrap(Wrap { trim: true })
        .scroll((app.tab_scroll as u16, 0))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(ACCENT))
                .title(Line::from(Span::styled(
                    " description (Ctrl-d/u scroll) ",
                    Style::default().fg(ACCENT),
                ))),
        );
    f.render_widget(p, area);
}

fn draw_tab_thread(f: &mut Frame<'_>, app: &mut App, area: Rect) {
    let main = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(22), Constraint::Min(6)])
        .split(area);

    let mut reply_parent: HashMap<CommentId, CommentId> = HashMap::new();
    for it in &app.thread_items {
        if let ThreadItem::Review {
            id,
            in_reply_to: Some(pid),
            ..
        } = it
        {
            reply_parent.insert(*id, *pid);
        }
    }
    let reply_depth = |id: CommentId| -> usize {
        let mut d = 0;
        let mut cur = id;
        while let Some(&p) = reply_parent.get(&cur) {
            d += 1;
            cur = p;
            if d > 64 {
                break;
            }
        }
        d
    };

    let items: Vec<ListItem> = app
        .thread_items
        .iter()
        .map(|it| {
            let prefix = match it {
                ThreadItem::Issue { .. } => ("conv", GREEN),
                ThreadItem::Review { .. } => ("file", PEACH),
            };
            let ind = match it {
                ThreadItem::Review { id, .. } => reply_depth(*id),
                _ => 0,
            };
            let indent = "  ".repeat(ind);
            let one_line: String = it
                .body()
                .lines()
                .next()
                .unwrap_or("")
                .chars()
                .take(100)
                .collect();
            let path_hint = match it {
                ThreadItem::Review { path, line, .. } => {
                    format!(" {}:{:?} ", path, line)
                }
                _ => String::new(),
            };
            let line = Line::from(vec![
                Span::styled(
                    format!("{indent}[{}] ", prefix.0),
                    Style::default().fg(prefix.1),
                ),
                Span::styled(
                    format!("{}  ", it.author()),
                    Style::default().fg(MAUVE),
                ),
                Span::styled(path_hint, Style::default().fg(SUB)),
                Span::styled(one_line, Style::default().fg(SUB)),
            ]);
            ListItem::new(line)
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(GREEN))
                .title(Line::from(Span::styled(
                    " thread  j/k  L reactions  [ ] hunk scroll  E $EDITOR ",
                    Style::default().fg(GREEN),
                ))),
        )
        .highlight_style(
            Style::default()
                .bg(SURFACE)
                .fg(TEXT)
                .add_modifier(Modifier::BOLD),
        );
    if app.thread_items.is_empty() {
        app.thread_list_state.select(None);
    } else {
        let i = app
            .thread_cursor
            .min(app.thread_items.len().saturating_sub(1));
        app.thread_list_state.select(Some(i));
    }
    f.render_stateful_widget(list, main[0], &mut app.thread_list_state);

    // Code context (review diff hunk) + markdown body: side-by-side when wide.
    let bottom = if main[1].width >= 92 {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(48), Constraint::Percentage(52)])
            .split(main[1])
    } else {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(45), Constraint::Min(4)])
            .split(main[1])
    };

    let hunk_area = bottom[0];
    let body_area = bottom[1];

    let (hunk_title, hunk_text) = app
        .thread_items
        .get(app.thread_cursor)
        .map(thread_hunk_pane)
        .unwrap_or((String::from("—"), String::from("Select a comment")));

    let hunk_widget = Paragraph::new(hunk_text.as_str())
        .style(Style::default().fg(SUB))
        .wrap(Wrap { trim: false })
        .scroll((app.thread_hunk_scroll as u16, 0))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(PEACH))
                .title(Line::from(Span::styled(
                    format!(" {hunk_title} "),
                    Style::default().fg(PEACH),
                ))),
        );
    f.render_widget(hunk_widget, hunk_area);

    let w = body_area.width.max(16) as usize;
    let detail_src = app
        .thread_items
        .get(app.thread_cursor)
        .map(thread_body_markdown)
        .unwrap_or_else(|| "Select a comment".into());
    let mut body = markdown_render::markdown_to_text(&detail_src, w);
    if let Some(r) = app.reactions_line.as_ref() {
        body.lines.push(Line::default());
        body.lines.push(Line::from(vec![Span::styled(
            "— reactions —",
            Style::default().fg(MAUVE).bold(),
        )]));
        body.lines.push(Line::from(vec![Span::styled(
            r.as_str(),
            Style::default().fg(GREEN),
        )]));
    }

    let p = Paragraph::new(body)
        .wrap(Wrap { trim: true })
        .scroll((app.thread_detail_scroll as u16, 0))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(MAUVE))
                .title(Line::from(Span::styled(
                    " comment (markdown)  Ctrl-d/u ",
                    Style::default().fg(MAUVE),
                ))),
        );
    f.render_widget(p, body_area);
}

/// Title + raw diff hunk for the middle pane (no markdown parse — faster).
fn thread_hunk_pane(it: &ThreadItem) -> (String, String) {
    match it {
        ThreadItem::Issue { .. } => (
            "conversation".into(),
            "(No file diff — this is an issue comment.)\n\nReplies and review threads on the left."
                .into(),
        ),
        ThreadItem::Review {
            path,
            line,
            diff_hunk,
            ..
        } => {
            let title = format!("@{path}:{line:?}");
            let h: String = diff_hunk.chars().take(12_000).collect();
            if h.trim().is_empty() {
                (title, "(empty diff hunk)".into())
            } else {
                (title, h)
            }
        }
    }
}

/// Markdown body only (diff hunk is shown separately for performance + readability).
fn thread_body_markdown(it: &ThreadItem) -> String {
    match it {
        ThreadItem::Issue { body, created, .. } => {
            format!("{created}\n\n{body}")
        }
        ThreadItem::Review {
            body,
            path,
            line,
            created,
            ..
        } => {
            let ln = line
                .map(|n| n.to_string())
                .unwrap_or_else(|| "?".into());
            format!("{created}\n`{path}` L{ln}\n\n{body}")
        }
    }
}

fn draw_tab_commits(f: &mut Frame<'_>, app: &mut App, area: Rect) {
    let items: Vec<ListItem> = app
        .commits
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let sel = i == app.commit_cursor;
            let msg: String = c.commit.message.lines().next().unwrap_or("").to_string();
            let line = Line::from(vec![
                Span::styled(
                    format!(
                        "{}  ",
                        c.sha.chars().take(7).collect::<String>()
                    ),
                    Style::default().fg(ACCENT),
                ),
                Span::styled(msg, Style::default().fg(if sel { TEXT } else { SUB })),
            ]);
            ListItem::new(line).style(if sel {
                Style::default().bg(SURFACE)
            } else {
                Style::default()
            })
        })
        .collect();
    f.render_widget(
        List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(ACCENT))
                .title(" commits "),
        ),
        area,
    );
}

fn draw_tab_files(f: &mut Frame<'_>, app: &mut App, area: Rect) {
    let items: Vec<ListItem> = app
        .files_lines
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let sel = i == app.file_cursor;
            ListItem::new(Span::styled(
                s.as_str(),
                Style::default().fg(if sel { TEXT } else { SUB }),
            ))
            .style(if sel {
                Style::default().bg(SURFACE)
            } else {
                Style::default()
            })
        })
        .collect();
    f.render_widget(
        List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(PEACH))
                .title(" files "),
        ),
        area,
    );
}

fn draw_tab_diff(f: &mut Frame<'_>, app: &mut App, area: Rect) {
    let p = Paragraph::new(app.diff_text.as_str())
        .wrap(Wrap { trim: false })
        .scroll((app.diff_scroll as u16, 0))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(RED))
                .title(" diff — Ctrl-d/u scroll · E opens in $VISUAL/$EDITOR (loads if empty) "),
        );
    f.render_widget(p, area);
}

fn draw_tab_reviews(f: &mut Frame<'_>, app: &mut App, area: Rect) {
    if app.reviews_composer.is_some() {
        draw_reviews_composer(f, app, area);
        return;
    }
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(4), Constraint::Length(2)])
        .split(area);
    let items: Vec<ListItem> = app
        .reviews_lines
        .iter()
        .map(|s| {
            ListItem::new(Span::styled(
                s.as_str(),
                Style::default().fg(SUB),
            ))
        })
        .collect();
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(MAUVE))
                .title(Line::from(Span::styled(
                    " submitted reviews  j/k  ·  a split-pane composer ",
                    Style::default().fg(MAUVE),
                ))),
        )
        .highlight_style(
            Style::default()
                .bg(SURFACE)
                .fg(TEXT)
                .add_modifier(Modifier::BOLD),
        );
    if app.reviews_lines.is_empty() {
        app.review_list_state.select(None);
    } else {
        let i = app
            .review_cursor
            .min(app.reviews_lines.len().saturating_sub(1));
        app.review_list_state.select(Some(i));
    }
    f.render_stateful_widget(list, chunks[0], &mut app.review_list_state);
    let hint = Paragraph::new(Line::from(vec![
        Span::styled(" a ", Style::default().fg(GREEN).bold()),
        Span::styled(
            "opens a 3-pane TUI here (Files | Diff | Finish) — same tab, no popup.  ",
            Style::default().fg(SUB),
        ),
        Span::styled("Esc", Style::default().fg(ACCENT)),
        Span::styled(" closes composer.", Style::default().fg(SUB)),
    ]))
    .style(Style::default().bg(BG));
    f.render_widget(hint, chunks[1]);
}

fn split_unicode_str_at(s: &str, char_idx: usize) -> (String, String) {
    if s.is_empty() {
        return (String::new(), String::new());
    }
    let mut count = 0usize;
    for (byte_i, _) in s.char_indices() {
        if count == char_idx {
            return (s[..byte_i].to_string(), s[byte_i..].to_string());
        }
        count += 1;
    }
    (s.to_string(), String::new())
}

fn draft_cursor_row_col(chars: &[char], cursor: usize) -> (usize, usize) {
    let mut row = 0usize;
    let mut col = 0usize;
    for (i, c) in chars.iter().enumerate() {
        if i >= cursor {
            break;
        }
        if *c == '\n' {
            row += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    (row, col)
}

fn char_lines_for_display(chars: &[char]) -> Vec<String> {
    if chars.is_empty() {
        return vec![String::new()];
    }
    let mut lines = Vec::new();
    let mut cur = String::new();
    for c in chars {
        if *c == '\n' {
            lines.push(cur);
            cur = String::new();
        } else {
            cur.push(*c);
        }
    }
    lines.push(cur);
    lines
}

/// GitHub-style gutter: old │ new │ marker + blue “+” when the row is selected and commentable.
fn review_diff_list_item(dl: &DiffDisplayLine, body_budget: usize, selected: bool) -> ListItem<'static> {
    match dl.kind {
        DiffLineKind::HunkHeader => {
            let t = PrListEntry::ellipsize(dl.body.as_str(), body_budget.max(8));
            ListItem::new(Line::from(Span::styled(
                t,
                Style::default().fg(MAUVE).italic(),
            )))
        }
        DiffLineKind::OutsideHunk => {
            let t = PrListEntry::ellipsize(dl.body.as_str(), body_budget.max(8));
            ListItem::new(Line::from(Span::styled(t, Style::default().fg(SUB))))
        }
        _ => {
            let old_s = dl
                .old_num
                .map(|n| format!("{n:>4}"))
                .unwrap_or_else(|| "    ".to_string());
            let new_s = dl
                .new_num
                .map(|n| format!("{n:>4}"))
                .unwrap_or_else(|| "    ".to_string());
            let marker = dl.marker_char();
            let mk_fg = match dl.kind {
                DiffLineKind::Removed => RED,
                DiffLineKind::Added => GREEN,
                _ => SUB,
            };
            let (body_fg, body_bg) = match dl.kind {
                DiffLineKind::Removed => (Color::Rgb(242, 200, 205), DIFF_DEL_BG),
                DiffLineKind::Added => (Color::Rgb(190, 230, 200), DIFF_ADD_BG),
                _ => (TEXT, BG),
            };
            let body = PrListEntry::ellipsize(dl.body.as_str(), body_budget.max(8));
            let pin = if dl.anchor.is_some() {
                if selected {
                    Span::styled(
                        "+",
                        Style::default()
                            .fg(BG)
                            .bg(ACCENT)
                            .add_modifier(Modifier::BOLD),
                    )
                } else {
                    Span::styled("·", Style::default().fg(ACCENT))
                }
            } else {
                Span::raw(" ")
            };
            ListItem::new(Line::from(vec![
                Span::styled(old_s, Style::default().fg(SUB)),
                Span::styled("│", Style::default().fg(SURFACE)),
                Span::styled(new_s, Style::default().fg(SUB)),
                Span::styled("│", Style::default().fg(SURFACE)),
                Span::styled(
                    format!("{marker} "),
                    Style::default().fg(mk_fg),
                ),
                pin,
                Span::styled(" ", Style::default()),
                Span::styled(
                    body,
                    Style::default().fg(body_fg).bg(body_bg),
                ),
            ]))
        }
    }
}

fn draw_inline_comment_draft(f: &mut Frame<'_>, area: Rect, draft: &InlineCommentDraft) {
    let path_short = PrListEntry::ellipsize(draft.path.as_str(), 24);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT))
        .title(Line::from(vec![
            Span::styled(" Write ", Style::default().fg(ACCENT).bold()),
            Span::styled(
                format!("{path_short}  L{} {}  ", draft.line, draft.side),
                Style::default().fg(SUB),
            ),
            Span::styled("Ctrl+Enter", Style::default().fg(GREEN)),
            Span::styled(" · ", Style::default().fg(SUB)),
            Span::styled("Esc", Style::default().fg(PEACH)),
            Span::styled(" · ", Style::default().fg(SUB)),
            Span::styled("Ctrl+e", Style::default().fg(MAUVE)),
        ]));
    let inner = block.inner(area);
    f.render_widget(block, area);

    if draft.chars.is_empty() {
        let line = Line::from(vec![
            Span::styled(
                "Leave a comment (Markdown). Use ```suggestion``` for proposed edits (same as github.com). ",
                Style::default().fg(SUB).italic(),
            ),
            Span::styled("▍", Style::default().fg(ACCENT).bold()),
        ]);
        f.render_widget(Paragraph::new(line).wrap(Wrap { trim: true }), inner);
        return;
    }

    let (cur_row, cur_col) = draft_cursor_row_col(&draft.chars, draft.cursor);
    let lines = char_lines_for_display(&draft.chars);
    let max_vis = inner.height.saturating_sub(1).max(1) as usize;
    let start = lines.len().saturating_sub(max_vis);
    let w = inner.width.saturating_sub(2) as usize;

    let mut text_lines: Vec<Line> = Vec::new();
    for (vis_i, li) in lines[start..].iter().enumerate() {
        let ri = start + vis_i;
        if ri == cur_row {
            let (a, b) = split_unicode_str_at(li, cur_col);
            text_lines.push(Line::from(vec![
                Span::styled(a, Style::default().fg(TEXT)),
                Span::styled("▍", Style::default().fg(ACCENT).bold()),
                Span::styled(b, Style::default().fg(TEXT)),
            ]));
        } else {
            let ell = PrListEntry::ellipsize(li.as_str(), w.max(8));
            text_lines.push(Line::from(Span::styled(
                ell,
                Style::default().fg(TEXT),
            )));
        }
    }

    f.render_widget(
        Paragraph::new(Text::from(text_lines)).wrap(Wrap { trim: false }),
        inner,
    );
}

/// Full-tab pending-review UI: three panes + tab strip (not a modal).
fn draw_reviews_composer(f: &mut Frame<'_>, app: &mut App, area: Rect) {
    let mut tmp = app.reviews_composer.take();
    let Some(comp) = tmp.as_mut() else {
        app.reviews_composer = tmp;
        return;
    };

    let strip_h = 2u16;
    let foot_h = 1u16;
    let outer = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(GREEN))
        .title(Line::from(vec![
            Span::styled(" review composer ", Style::default().fg(GREEN).bold()),
            Span::styled(
                format!("pending #{}  ", comp.pending_review_id),
                Style::default().fg(MAUVE),
            ),
            Span::styled(
                format!("{}  ", &comp.commit_sha[..comp.commit_sha.len().min(7)]),
                Style::default().fg(SUB),
            ),
        ]));
    let inner = outer.inner(area);
    f.render_widget(outer, area);
    let v = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(strip_h),
            Constraint::Min(4),
            Constraint::Length(foot_h),
        ])
        .split(inner);

    if comp.subphase == ReviewsComposerSubphase::ConfirmDiscard {
        let tab_strip = pane_strip_line(comp.focus);
        f.render_widget(Paragraph::new(tab_strip), v[0]);
        let txt = Text::from(vec![
            Line::from(""),
            Line::from(
                Span::styled(
                    "Discard pending review on GitHub?",
                    Style::default().fg(RED).bold(),
                ),
            ),
            Line::from(""),
            Line::from(Span::styled(
                "Draft inline comments in this review will be removed.",
                Style::default().fg(SUB),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "y  yes     n / Esc  cancel",
                Style::default().fg(TEXT),
            )),
        ]);
        f.render_widget(
            Paragraph::new(txt).alignment(Alignment::Center),
            v[1],
        );
        f.render_widget(
            Paragraph::new(Span::styled(
                " confirm discard ",
                Style::default().fg(RED),
            )),
            v[2],
        );
        app.reviews_composer = tmp;
        return;
    }

    f.render_widget(Paragraph::new(pane_strip_line(comp.focus)), v[0]);

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(26),
            Constraint::Percentage(49),
            Constraint::Percentage(25),
        ])
        .split(v[1]);

    let b_files = pane_border(comp.focus == ReviewsComposePane::Files, ACCENT);
    let files_block = Block::default()
        .borders(Borders::ALL)
        .border_style(b_files)
        .title(Line::from(Span::styled(
            " ① files ",
            Style::default().fg(if comp.focus == ReviewsComposePane::Files {
                GREEN
            } else {
                SUB
            }),
        )));
    let fi = files_block.inner(body[0]);
    app.compose_hit_files.set(Some(fi));
    let file_items: Vec<ListItem> = app
        .file_paths
        .iter()
        .map(|p| {
            ListItem::new(Span::styled(
                p.as_str(),
                Style::default().fg(SUB),
            ))
        })
        .collect();
    let flist = List::new(file_items)
        .block(files_block)
        .highlight_style(
            Style::default()
                .bg(SURFACE)
                .fg(TEXT)
                .add_modifier(Modifier::BOLD),
        );
    if app.file_paths.is_empty() {
        app.inline_review_file_state.select(None);
    } else {
        let i = comp
            .file_cursor
            .min(app.file_paths.len().saturating_sub(1));
        app.inline_review_file_state.select(Some(i));
    }
    f.render_stateful_widget(flist, body[0], &mut app.inline_review_file_state);

    let b_diff = pane_border(comp.focus == ReviewsComposePane::Diff, PEACH);
    let mid_col = body[1];
    let split_mid = if comp.comment_draft.is_some() {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(6), Constraint::Min(5)])
            .split(mid_col)
    } else {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(100), Constraint::Length(0)])
            .split(mid_col)
    };
    let code_area = split_mid[0];
    let draft_area = if comp.comment_draft.is_some() {
        Some(split_mid[1])
    } else {
        None
    };

    let diff_title = if comp.path.is_empty() {
        " ② files changed (pick in ①) ".to_string()
    } else if comp.comment_draft.is_some() {
        format!(" ② {} — pick line, + = comment ", comp.path)
    } else {
        format!(" ② {} ", comp.path)
    };
    let diff_block = Block::default()
        .borders(Borders::ALL)
        .border_style(b_diff)
        .title(Line::from(Span::styled(
            diff_title,
            Style::default().fg(if comp.focus == ReviewsComposePane::Diff {
                PEACH
            } else {
                SUB
            }),
        )));
    let di = diff_block.inner(code_area);
    app.compose_hit_diff.set(Some(di));
    let body_budget = di.width.saturating_sub(2).saturating_sub(18) as usize;
    let diff_items: Vec<ListItem> = comp
        .diff_lines
        .iter()
        .enumerate()
        .map(|(idx, ln)| {
            review_diff_list_item(
                ln,
                body_budget.max(12),
                idx == comp.line_cursor,
            )
        })
        .collect();
    let hl = if comp.comment_draft.is_some() {
        Style::default().bg(SURFACE).fg(SUB)
    } else {
        Style::default()
            .bg(SURFACE)
            .fg(GREEN)
            .add_modifier(Modifier::BOLD)
    };
    let dlist = List::new(diff_items).block(diff_block).highlight_style(hl);
    if comp.diff_lines.is_empty() {
        app.inline_review_line_state.select(None);
    } else {
        let i = comp
            .line_cursor
            .min(comp.diff_lines.len().saturating_sub(1));
        app.inline_review_line_state.select(Some(i));
    }
    f.render_stateful_widget(dlist, code_area, &mut app.inline_review_line_state);

    if let (Some(area), Some(draft)) = (draft_area, comp.comment_draft.as_ref()) {
        draw_inline_comment_draft(f, area, draft);
    }

    let act_body = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(42), Constraint::Percentage(58)])
        .split(body[2]);

    let b_act = pane_border(comp.focus == ReviewsComposePane::Actions, MAUVE);
    let sess_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(SUB))
        .title(Line::from(Span::styled(
            format!(" session ({}) ", comp.session_comments.len()),
            Style::default().fg(SUB),
        )));
    let si = sess_block.inner(act_body[0]);
    f.render_widget(sess_block, act_body[0]);
    let sess_txt = if comp.session_comments.is_empty() {
        Text::from(Line::from(Span::styled(
            "No comments in this session yet — in ② press Enter on a + line to write.",
            Style::default().fg(SUB).italic(),
        )))
    } else {
        Text::from(
            comp.session_comments
                .iter()
                .map(|s| {
                    Line::from(Span::styled(
                        PrListEntry::ellipsize(s, si.width.saturating_sub(2) as usize),
                        Style::default().fg(TEXT),
                    ))
                })
                .collect::<Vec<_>>(),
        )
    };
    f.render_widget(
        Paragraph::new(sess_txt).wrap(Wrap { trim: true }),
        si,
    );

    let finish_block = Block::default()
        .borders(Borders::ALL)
        .border_style(b_act)
        .title(Line::from(Span::styled(
            " ③ finish ",
            Style::default().fg(if comp.focus == ReviewsComposePane::Actions {
                MAUVE
            } else {
                SUB
            }),
        )));
    let fai = finish_block.inner(act_body[1]);
    app.compose_hit_actions.set(Some(fai));
    let labels = [
        " Approve (instant)",
        " Request changes ($EDITOR)",
        " Comment ($EDITOR)",
        " Discard pending…",
    ];
    let items: Vec<ListItem> = labels
        .iter()
        .map(|l| ListItem::new(Span::styled(*l, Style::default().fg(TEXT))))
        .collect();
    let alist = List::new(items)
        .block(finish_block)
        .highlight_style(
            Style::default()
                .bg(SURFACE)
                .fg(MAUVE)
                .add_modifier(Modifier::BOLD),
        );
    let si_sel = comp.submit_cursor.min(3);
    app.inline_review_submit_state.select(Some(si_sel));
    f.render_stateful_widget(alist, act_body[1], &mut app.inline_review_submit_state);

    let footer = match comp.focus {
        ReviewsComposePane::Files => {
            " j/k move · Enter load diff into ② · Tab next pane · mouse click pane "
        }
        ReviewsComposePane::Diff => {
            if comp.comment_draft.is_some() {
                " comment box focused · Ctrl+Enter post · Esc cancel · Ctrl+e $EDITOR "
            } else {
                " j/k · n/p commentable · Enter open write box (GitHub-style) · Ctrl+e $EDITOR · Tab "
            }
        }
        ReviewsComposePane::Actions => {
            " j/k · Enter run · Tab next · Esc leave composer (draft stays on GitHub) "
        }
    };
    f.render_widget(
        Paragraph::new(Span::styled(footer, Style::default().fg(SUB))),
        v[2],
    );

    app.reviews_composer = tmp;
}

fn pane_border(focused: bool, accent: Color) -> Style {
    if focused {
        Style::default().fg(accent).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(SURFACE)
    }
}

fn pane_strip_line(focus: ReviewsComposePane) -> Line<'static> {
    let mk = |p: ReviewsComposePane, label: &'static str| {
        let on = focus == p;
        Span::styled(
            label,
            Style::default()
                .fg(if on { GREEN } else { SUB })
                .add_modifier(if on {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
        )
    };
    Line::from(vec![
        mk(ReviewsComposePane::Files, " Files "),
        Span::styled(" │ ", Style::default().fg(SUB)),
        mk(ReviewsComposePane::Diff, " Diff "),
        Span::styled(" │ ", Style::default().fg(SUB)),
        mk(ReviewsComposePane::Actions, " Finish "),
        Span::styled(
            "     Tab · Shift+Tab · Esc close ",
            Style::default().fg(SUB),
        ),
    ])
}

fn draw_overlay(f: &mut Frame<'_>, app: &mut App, full: Rect) {
    match &mut app.overlay {
        Overlay::None => {}
        Overlay::Help => {
            let w = (full.width * 4 / 5).max(40);
            let h = (full.height * 4 / 5).max(20);
            let x = (full.width.saturating_sub(w)) / 2;
            let y = (full.height.saturating_sub(h)) / 2;
            let area = Rect { x, y, width: w, height: h };
            f.render_widget(Clear, area);
            let help = HELP_TEXT;
            let p = Paragraph::new(help)
                .wrap(Wrap { trim: true })
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(ACCENT))
                        .title(Line::from(Span::styled(
                            " help (q Esc ? close) ",
                            Style::default().fg(ACCENT).bold(),
                        )))
                        .style(Style::default().bg(SURFACE).fg(TEXT)),
                );
            f.render_widget(p, area);
        }
        Overlay::FilterSummary(FilterPanelPhase::Overview) => {
            let w = (full.width * 4 / 5).max(50);
            let h = (full.height * 3 / 5).max(18);
            let x = (full.width.saturating_sub(w)) / 2;
            let y = (full.height.saturating_sub(h)) / 2;
            let area = Rect { x, y, width: w, height: h };
            f.render_widget(Clear, area);
            let p = Paragraph::new(app.filters_summary())
                .wrap(Wrap { trim: true })
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(GREEN))
                        .title(Line::from(Span::styled(
                            " filters (f)  s → status  Esc/q/click close ",
                            Style::default().fg(GREEN).bold(),
                        )))
                        .style(Style::default().bg(SURFACE).fg(TEXT)),
                );
            f.render_widget(p, area);
        }
        Overlay::FilterSummary(FilterPanelPhase::StatusPick { cursor }) => {
            let w = 40;
            let h = 11u16;
            let x = (full.width.saturating_sub(w)) / 2;
            let y = (full.height.saturating_sub(h)) / 2;
            let area = Rect { x, y, width: w, height: h };
            f.render_widget(Clear, area);
            let opts = ["open", "closed", "merged", "draft", "all"];
            let cur = *cursor;
            let items: Vec<ListItem> = opts
                .iter()
                .enumerate()
                .map(|(i, label)| {
                    let sel = i == cur;
                    ListItem::new(Span::styled(
                        *label,
                        Style::default().fg(if sel { GREEN } else { TEXT }),
                    ))
                    .style(if sel {
                        Style::default().bg(BG)
                    } else {
                        Style::default()
                    })
                })
                .collect();
            f.render_widget(
                List::new(items).block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(ACCENT))
                        .title(" status  j/k Enter apply  Esc → back  q close ")
                        .style(Style::default().bg(SURFACE)),
                ),
                area,
            );
        }
        Overlay::Command => {
            let w = full.width.saturating_sub(4);
            let h = 3u16;
            let area = Rect {
                x: 2,
                y: full.height.saturating_sub(h + 2),
                width: w,
                height: h,
            };
            f.render_widget(Clear, area);
            let p = Paragraph::new(format!(":{}{}", app.command_buf, "▏"))
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(MAUVE))
                        .title(" command ")
                        .style(Style::default().bg(SURFACE).fg(TEXT)),
                );
            f.render_widget(p, area);
        }
        Overlay::ReactionPicker => {
            let w = 36;
            let h = 14u16;
            let x = (full.width.saturating_sub(w)) / 2;
            let y = (full.height.saturating_sub(h)) / 2;
            let area = Rect { x, y, width: w, height: h };
            f.render_widget(Clear, area);
            use octocrab::models::reactions::ReactionContent;
            let opts: &[(&str, ReactionContent)] = &[
                ("+1  thumbs up", ReactionContent::PlusOne),
                ("-1  thumbs down", ReactionContent::MinusOne),
                ("laugh", ReactionContent::Laugh),
                ("confused", ReactionContent::Confused),
                ("heart", ReactionContent::Heart),
                ("hooray", ReactionContent::Hooray),
                ("rocket", ReactionContent::Rocket),
                ("eyes", ReactionContent::Eyes),
            ];
            let items: Vec<ListItem> = opts
                .iter()
                .enumerate()
                .map(|(i, (label, _))| {
                    let sel = i == app.reaction_cursor;
                    ListItem::new(Span::styled(
                        *label,
                        Style::default().fg(if sel { GREEN } else { TEXT }),
                    ))
                    .style(if sel {
                        Style::default().bg(BG)
                    } else {
                        Style::default()
                    })
                })
                .collect();
            f.render_widget(
                List::new(items).block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(PEACH))
                        .title(" reaction (+) j/k Enter ")
                        .style(Style::default().bg(SURFACE)),
                ),
                area,
            );
        }
        Overlay::CreatePrWizard {
            phase,
            title,
            head,
            base,
            buf,
        } => {
            let w = (full.width * 3 / 4).max(50);
            let h = 12u16;
            let x = (full.width.saturating_sub(w)) / 2;
            let y = (full.height.saturating_sub(h)) / 2;
            let area = Rect { x, y, width: w, height: h };
            f.render_widget(Clear, area);
            let prompt = match *phase {
                0 => "PR title",
                1 => "head branch (your branch)",
                2 => "base branch (target)",
                _ => "?",
            };
            let done = format!("title: {title}\nhead: {head}\nbase: {base}\n");
            let text = format!("{done}\n{prompt}:\n{buf}▏");
            let p = Paragraph::new(text).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(GREEN))
                    .title(" new PR — head/base prefilled from git when possible ")
                    .style(Style::default().bg(SURFACE).fg(TEXT)),
            );
            f.render_widget(p, area);
        }
    }
}

const HELP_TEXT: &str = "\
gh-pr-cli — quick reference\n\
═══════════════════════════════════════════════════════════════════\n\
\n\
AUTH\n\
  gh auth login     (preferred)    or    env GITHUB_TOKEN\n\
\n\
PR LIST\n\
  j k Enter     move / open PR          Mouse click   select row\n\
  f             filters (s = status)    A             status menu only\n\
  a             cycle status            m r           more / refresh page 1\n\
  n             new PR wizard           o             browser\n\
  : :create :pr same wizard — head = current git branch, base = origin default or main\n\
\n\
PR DETAIL (tabs 1-6)\n\
  q             back to list\n\
  E             OPEN CURRENT TAB IN $VISUAL / $EDITOR\n\
                  - Diff tab: full patch (.diff), fetched first if empty\n\
                  - Thread: selected comment text + diff hunk\n\
                  - Info / Reviews / Commits / Files: that tab’s buffer\n\
  V             Neovim: threaded comments + full diff (binary: GH_PR_CLI_NVIM)\n\
  Ctrl+d u      scroll the focused pane (Info, Thread body, Diff, …)\n\
\n\
THREAD (tab 2) — review with code context\n\
  j k           pick comment (list scrolls with selection)\n\
  Center area   left/top = diff at comment, right/bottom = markdown + reactions\n\
  [  ]          scroll the code / diff-hunk pane\n\
  c R e d       new comment / reply / edit / delete\n\
  + L           reaction picker / load counts (cached after L)\n\
  I             Kitty: kitten icat for first image URL in body\n\
  g g  G        scroll comment body top / bottom\n\
\n\
REVIEWS (tab 6)\n\
  j k           browse submitted reviews (when composer is closed)\n\
  a             open split-pane composer (Files | Diff | Finish), like “Start a review”\n\
                Tab / Shift+Tab   panes · Esc closes composer (pending review stays on GitHub)\n\
                Diff: GitHub-style file view — old│new│+/- gutters; n/p jump commentable lines\n\
                Enter         docked “Write” box under the diff (Markdown; Ctrl+Enter posts comment)\n\
                Ctrl+e        open $EDITOR for long comments · Esc closes draft only\n\
                Finish pane: Approve | Request changes | Comment | Discard (y confirm)\n\
                Mouse selects pane/row when no draft is open\n\
\n\
FILTERS  (:command from any screen, refreshes list)\n\
  :filter clear | show    :state open|closed|merged|draft|all\n\
  :author :assignee :mentions :reviewer :reviewed :label :title :head :base\n\
  Each :field clear   Branch-only uses REST; other fields use GitHub search.\n\
\n\
STARTUP\n\
  --status STATE              GH_PR_CLI_STATUS  GH_PR_CLI_TITLE  …  (see :filter show)\n\
\n\
";
