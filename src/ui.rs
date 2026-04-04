//! Layout and styling (Catppuccin-inspired palette on dark backgrounds).

use crate::app::{App, FilterPanelPhase, Overlay, PrListEntry, PrTab, Screen, ThreadItem};
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

pub fn draw(f: &mut Frame<'_>, app: &App) {
    app.pr_list_hit_rect.set(None);
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

fn draw_header(f: &mut Frame<'_>, app: &App, area: Rect) {
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

fn draw_status(f: &mut Frame<'_>, app: &App, area: Rect) {
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

fn draw_pr_list(f: &mut Frame<'_>, app: &App, area: Rect) {
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

fn draw_pr_detail(f: &mut Frame<'_>, app: &App, area: Rect) {
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
    if let Some(pr) = &app.current_pr {
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
            PrTab::Info => draw_tab_info(f, app, pr, v[1]),
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

fn draw_tab_info(f: &mut Frame<'_>, app: &App, pr: &octocrab::models::pulls::PullRequest, area: Rect) {
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

fn draw_tab_thread(f: &mut Frame<'_>, app: &App, area: Rect) {
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
        .enumerate()
        .map(|(i, it)| {
            let sel = i == app.thread_cursor;
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
                Span::styled(one_line, Style::default().fg(if sel { TEXT } else { SUB })),
            ]);
            let style = if sel {
                Style::default().bg(SURFACE)
            } else {
                Style::default()
            };
            ListItem::new(line).style(style)
        })
        .collect();

    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(GREEN))
            .title(Line::from(Span::styled(
                " thread  j/k  L reactions  [ ] hunk scroll  E $EDITOR ",
                Style::default().fg(GREEN),
            ))),
    );
    f.render_widget(list, main[0]);

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

fn draw_tab_commits(f: &mut Frame<'_>, app: &App, area: Rect) {
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

fn draw_tab_files(f: &mut Frame<'_>, app: &App, area: Rect) {
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

fn draw_tab_diff(f: &mut Frame<'_>, app: &App, area: Rect) {
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

fn draw_tab_reviews(f: &mut Frame<'_>, app: &App, area: Rect) {
    let items: Vec<ListItem> = app
        .reviews_lines
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let sel = i == app.review_cursor;
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
                .border_style(Style::default().fg(MAUVE))
                .title(" reviews "),
        ),
        area,
    );
}

fn draw_overlay(f: &mut Frame<'_>, app: &App, full: Rect) {
    match &app.overlay {
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
            let items: Vec<ListItem> = opts
                .iter()
                .enumerate()
                .map(|(i, label)| {
                    let sel = i == *cursor;
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
  j k           pick comment (no API while moving — fast)\n\
  Center area   left/top = diff at comment, right/bottom = markdown + reactions\n\
  [  ]          scroll the code / diff-hunk pane\n\
  c R e d       new comment / reply / edit / delete\n\
  + L           reaction picker / load counts (cached after L)\n\
  I             Kitty: kitten icat for first image URL in body\n\
  g g  G        scroll comment body top / bottom\n\
\n\
FILTERS  (:command from any screen, refreshes list)\n\
  :filter clear | show    :state open|closed|merged|draft|all\n\
  :author :assignee :mentions :reviewer :reviewed :label :title :head :base\n\
  Each :field clear   Branch-only uses REST; other fields use GitHub search.\n\
\n\
STARTUP\n\
  --status STATE              GH_PR_CLI_STATUS  GH_PR_CLI_TITLE  …  (see :filter show)\n\
\n\
Not built yet: start-review with line-picked suggestions (use github.com or gh).\n\
";
