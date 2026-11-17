//! shared pieces themes compose. no layout decisions here.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, Padding, Paragraph, Scrollbar, ScrollbarOrientation,
    ScrollbarState,
};
use ratatui::Frame;

use crate::app::{self, App};

/// the muted rule colour used across layouts.
pub const RULE: Color = Color::Rgb(0x2a, 0x30, 0x3a);

/// the transcript for a pane, pinned to the newest content when following.
pub fn transcript(frame: &mut Frame<'_>, area: Rect, app: &mut App) {
    transcript_with(frame, area, app, std::convert::identity);
}

/// as [`transcript`], for the themes that decorate the paragraph themselves.
pub fn transcript_with<F>(frame: &mut Frame<'_>, area: Rect, app: &mut App, decorate: F)
where
    F: FnOnce(Paragraph<'static>) -> Paragraph<'static>,
{
    // the bar column is reserved even while the bar is hidden: reflowing the text as it
    // appears would invalidate the render cache and make the whole pane jump.
    let bar_column = area.width > 1;
    let text_area = if bar_column {
        Rect {
            width: area.width - 1,
            ..area
        }
    } else {
        area
    };
    let area = text_area;
    // rebuilding the whole transcript on every scroll keypress is what made scrolling
    // crawl, so reuse the render until the content or the width actually changes.
    let stale = {
        let tab = app.tab();
        tab.cache
            .as_ref()
            .is_none_or(|c| (c.content_rev != tab.content_rev) || (c.width != area.width))
    };
    if stale {
        let lines = {
            let tab = app.tab();
            app::transcript_lines(tab, area.width)
        };
        let tab = app.tab_mut();
        tab.cache = Some(app::Cache {
            content_rev: tab.content_rev,
            width: area.width,
            lines,
        });
    }

    let tab = app.tab_mut();
    let content = tab
        .cache
        .as_ref()
        .map(|c| c.lines.clone())
        .unwrap_or_default();
    let width = usize::from(area.width).max(1);
    let visual: usize = content
        .iter()
        .map(|line| {
            let w = line.width();
            if w == 0 {
                1
            } else {
                w.div_ceil(width)
            }
        })
        .sum();
    tab.max_scroll =
        u16::try_from(visual.saturating_sub(usize::from(area.height))).unwrap_or(u16::MAX);
    if tab.follow {
        tab.scroll = tab.max_scroll;
    } else {
        tab.scroll = tab.scroll.min(tab.max_scroll);
    }
    let scroll = tab.scroll;
    let span = tab.max_scroll;
    let visible = tab.scroll_touched.is_some() && span > 0;
    let paragraph = decorate(Paragraph::new(content).scroll((scroll, 0)));
    frame.render_widget(paragraph, area);

    if visible {
        let mut bar_state =
            ScrollbarState::new(usize::from(span)).position(usize::from(scroll.min(span)));
        let bar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .style(Style::default().fg(RULE))
            .thumb_symbol("\u{2503}")
            .track_symbol(Some(" "))
            .begin_symbol(None)
            .end_symbol(None);
        let bar_area = Rect {
            x: area.x + area.width,
            y: area.y,
            width: 1,
            height: area.height,
        };
        frame.render_stateful_widget(bar, bar_area, &mut bar_state);
    }
}

/// animation frames for the activity spinner.
const SPINNER: [&str; 10] = [
    "\u{280b}", "\u{2819}", "\u{2839}", "\u{2838}", "\u{283c}", "\u{2834}", "\u{2826}", "\u{2827}",
    "\u{2807}", "\u{280f}",
];

/// shortens a tab label while keeping its trailing `(session)` marker intact.
fn fit_label(label: &str, max: usize) -> String {
    if label.chars().count() <= max {
        return label.to_owned();
    }
    let tail = label
        .rfind('(')
        .map(|at| label[at..].to_owned())
        .unwrap_or_default();
    let room = max.saturating_sub(tail.chars().count() + 1);
    let head: String = label.chars().take(room).collect();
    format!("{head}\u{2026}{tail}")
}

/// the session tab strip.
#[must_use]
pub fn tab_bar(app: &App, width: u16) -> Line<'static> {
    const MAX_LABEL: usize = 28;

    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut used: usize = 0;

    for (index, tab) in app.tabs.iter().enumerate() {
        let busy = tab.status.turn_running;
        let mark = if busy {
            format!("{} ", SPINNER[app.spinner % SPINNER.len()])
        } else {
            String::new()
        };
        let mut label = fit_label(&format!("{mark}{} {}", index + 1, tab.title), MAX_LABEL);
        let cell = label.chars().count() + 2;
        let _ = &mut label;
        if used + cell + 3 > usize::from(width) {
            break;
        }
        if index > 0 {
            spans.push(Span::styled("\u{2502}", Style::default().fg(app::FAINT)));
            used += 1;
        }
        if index == app.active {
            spans.push(Span::styled(
                format!(" {label} "),
                Style::default()
                    .fg(Color::Rgb(0x10, 0x14, 0x1a))
                    .bg(app::ACCENT)
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            spans.push(Span::styled(
                format!(" {label} "),
                Style::default().fg(app::DIM),
            ));
        }
        used += cell;
    }

    spans.push(Span::styled(" \u{002b}", Style::default().fg(app::FAINT)));
    Line::from(spans)
}

/// a compact context meter: filled cells and a percentage, coloured by pressure.
fn context_meter(used: u64, limit: u64) -> (String, Color) {
    const CELLS: usize = 10;
    let pct = used
        .saturating_mul(100)
        .checked_div(limit)
        .unwrap_or(0)
        .min(100);
    let filled = (pct as usize * CELLS / 100).min(CELLS);
    let ink = if pct >= 85 {
        app::BAD
    } else if pct >= 60 {
        Color::Rgb(0xe0, 0xb0, 0x60)
    } else {
        app::OK
    };
    (
        format!(
            "{}{} {pct:>3}%",
            "\u{2588}".repeat(filled),
            "\u{2591}".repeat(CELLS - filled)
        ),
        ink,
    )
}

/// a path as the status bar shows it: home-relative, and cut from the left so the leaf
/// stays readable on a narrow terminal.
#[must_use]
pub fn short_path(path: &str) -> String {
    const MAX: usize = 40;
    let mut shown = path.to_owned();
    if let Some(home) = std::env::var_os("HOME") {
        let home = home.to_string_lossy().into_owned();
        if let Some(rest) = shown.strip_prefix(&home) {
            // only a whole component: /home/chrisaw2 is not inside /home/chrisaw.
            if rest.is_empty() || rest.starts_with('/') {
                shown = format!("~{rest}");
            }
        }
    }
    let count = shown.chars().count();
    if count > MAX {
        let tail: String = shown.chars().skip(count - (MAX - 1)).collect();
        shown = format!("\u{2026}{tail}");
    }
    shown
}

/// the powerline status bar.
#[must_use]
pub fn status_bar(app: &App, width: u16) -> Line<'static> {
    const SEP: &str = "\u{e0b0}";
    let tab = app.tab();
    let mut segments: Vec<(String, Color, Color)> = vec![(
        format!(" {} ", short_path(&tab.cwd)),
        Color::Rgb(0xd8, 0xd8, 0xd8),
        Color::Rgb(0x2a, 0x2a, 0x2a),
    )];
    if let Some(branch) = &app.branch {
        segments.push((
            format!(" \u{e725} {branch} "),
            Color::Rgb(0xd8, 0xd8, 0xf0),
            Color::Rgb(0x3b, 0x3b, 0x5c),
        ));
    }
    if !tab.status.model.is_empty() {
        let label = if tab.status.effort.is_empty() {
            tab.status.model.clone()
        } else {
            format!("{}:{}", tab.status.model, tab.status.effort)
        };
        segments.push((
            format!(" \u{f2db} {label} "),
            Color::Rgb(0xe0, 0xe8, 0xff),
            Color::Rgb(0x2f, 0x45, 0x66),
        ));
    }
    segments.push((
        format!(" \u{f0e4} {:.1} t/s ", tab.tok_per_sec),
        if tab.flash_highlight {
            Color::Rgb(0xff, 0xff, 0xff)
        } else {
            Color::Rgb(0xff, 0xd8, 0xd8)
        },
        if tab.flash_highlight {
            Color::Rgb(0xc0, 0x30, 0x30)
        } else {
            Color::Rgb(0x5c, 0x2f, 0x2f)
        },
    ));
    // context pressure, not raw throughput: how much of the window is in use.
    let (meter, ink) = context_meter(tab.status.context_tokens, tab.status.context_limit);
    segments.push((format!(" {meter} "), ink, Color::Rgb(0x2a, 0x2a, 0x2a)));

    let mut spans: Vec<Span<'static>> = Vec::new();
    for (index, (text, fg, bg)) in segments.iter().enumerate() {
        spans.push(Span::styled(text.clone(), Style::default().fg(*fg).bg(*bg)));
        let next = segments.get(index + 1).map_or(Color::Reset, |s| s.2);
        spans.push(Span::styled(SEP, Style::default().fg(*bg).bg(next)));
    }
    let used = Line::from(spans.clone()).width();

    // the hint is contextual: while a turn runs, interrupting is the only useful key, so it
    // takes the whole hint rather than sharing it.
    let busy = tab.status.turn_running;
    let hint = if busy {
        " esc interrupt "
    } else {
        " ctrl+t new \u{00b7} ctrl+w close \u{00b7} ctrl+1..9 jump "
    };
    let pad = usize::from(width).saturating_sub(used + hint.chars().count());
    spans.push(Span::raw(" ".repeat(pad)));
    spans.push(Span::styled(hint, Style::default().fg(app::FAINT)));
    Line::from(spans)
}

/// a bordered composer. returns where the text cursor belongs.
pub fn composer(frame: &mut Frame<'_>, area: Rect, app: &App) -> Option<(u16, u16)> {
    let tab = app.tab();
    let field = usize::from(area.width.saturating_sub(4));
    let (hscroll, column) = tab.input_view(field);
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(RULE))
        .padding(Padding::new(1, 1, 0, 0));

    // the in-progress header rides the composer's top border, where the operator is
    // already looking, and vanishes with the turn.
    if tab.status.turn_running {
        if let Some(word) = &tab.working {
            block = block.title(
                Line::from(format!(" {word}\u{2026} ")).style(Style::default().fg(app::ACCENT)),
            );
        }
    }

    frame.render_widget(
        Paragraph::new({
            let mut line = Line::from(Span::styled(
                tab.input_text().to_owned(),
                Style::default().fg(app::INK),
            ));
            // the rest of the completion, shown where it would land.
            if let Some(ghost) = tab.suggestion() {
                line.push_span(Span::styled(ghost, Style::default().fg(app::FAINT)));
            }
            line
        })
        .scroll((0, u16::try_from(hscroll).unwrap_or(u16::MAX)))
        .block(block),
        area,
    );
    Some((area.x + 2 + u16::try_from(column).unwrap_or(0), area.y + 1))
}

/// key/value facts about the session, for sidebars and dashboards.
#[must_use]
pub fn facts(app: &App) -> Vec<(&'static str, String)> {
    let tab = app.tab();
    let mut out = vec![
        ("session", tab.session.chars().take(8).collect::<String>()),
        ("cwd", short_path(&tab.cwd)),
        ("model", tab.status.model.clone()),
        ("effort", tab.status.effort.clone()),
        ("rate", format!("{:.1} t/s", tab.tok_per_sec)),
        (
            "tokens",
            format!(
                "{}/{}",
                compact(tab.status.prompt_tokens),
                compact(tab.status.completion_tokens)
            ),
        ),
    ];
    if let Some(branch) = &app.branch {
        out.insert(1, ("branch", branch.clone()));
    }
    out.retain(|(_, value)| !value.is_empty());
    out
}

/// abbreviates large counts.
#[must_use]
pub fn compact(count: u64) -> String {
    if count >= 1_000_000 {
        format!("{}M", count / 1_000_000)
    } else if count >= 1_000 {
        format!("{}k", count / 1_000)
    } else {
        count.to_string()
    }
}

/// a bordered pane with a title.
#[must_use]
pub fn pane(title: &str, edge: Color) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .title(format!(" {title} "))
        .border_style(Style::default().fg(edge))
}

/// draws the open picker over the interface. shared, so every theme gets it.
pub fn picker(frame: &mut Frame<'_>, app: &App) {
    let Some(picker) = &app.picker else {
        return;
    };
    let screen = frame.area();
    let width = screen
        .width
        .saturating_sub(8)
        .clamp(24, 72)
        .min(screen.width);
    let wanted = u16::try_from(picker.items.len().clamp(1, 12)).unwrap_or(12);
    let height = (wanted + 3).min(screen.height);
    let rect = Rect {
        x: screen.x + screen.width.saturating_sub(width) / 2,
        y: screen.y + screen.height.saturating_sub(height) / 2,
        width,
        height,
    };

    frame.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(app::ACCENT))
        .title(Line::from(format!(" {} ", picker.title)).style(Style::default().fg(app::ACCENT)));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    // keep the selection on screen without scrolling the whole list into view.
    let visible = usize::from(inner.height).max(1);
    let offset = picker.selected.saturating_sub(visible.saturating_sub(1));
    let lines: Vec<Line<'static>> = picker
        .items
        .iter()
        .enumerate()
        .skip(offset)
        .take(visible)
        .map(|(index, item)| {
            let chosen = index == picker.selected;
            let mark = if chosen { "\u{25b8} " } else { "  " };
            let style = if chosen {
                Style::default()
                    .fg(app::ACCENT)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(app::FAINT)
            };
            Line::from(format!("{mark}{}", item.label)).style(style)
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

/// draws the queued attachment over the interface, so the operator sees what is about to
/// be sent. skipped when the terminal has no image protocol.
pub fn attachment_preview(frame: &mut Frame<'_>, app: &mut App) {
    let Some(attachment) = app.tab_mut().attachments.last_mut() else {
        return;
    };
    let Some(protocol) = attachment.protocol.as_mut() else {
        return;
    };
    let screen = frame.area();
    let width = screen
        .width
        .saturating_sub(8)
        .clamp(16, 56)
        .min(screen.width);
    let height = screen
        .height
        .saturating_sub(6)
        .clamp(8, 22)
        .min(screen.height);
    let rect = Rect {
        x: screen.x + screen.width.saturating_sub(width) / 2,
        y: screen.y + screen.height.saturating_sub(height) / 2,
        width,
        height,
    };
    frame.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(app::ACCENT))
        .title(
            Line::from(format!(" {} ", attachment.name)).style(Style::default().fg(app::ACCENT)),
        );
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    frame.render_stateful_widget(ratatui_image::StatefulImage::default(), inner, protocol);
}
