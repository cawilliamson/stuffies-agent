//! shared pieces themes compose. no layout decisions here.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Padding, Paragraph};
use ratatui::Frame;

use crate::app::{self, App};

/// the muted rule colour used across layouts.
pub const RULE: Color = Color::Rgb(0x2a, 0x30, 0x3a);

/// the transcript for a pane, pinned to the newest content when following.
pub fn transcript(app: &mut App, area: Rect) -> Paragraph<'static> {
    // rebuilding the whole transcript on every scroll keypress is what made scrolling
    // crawl, so reuse the render until the content or the width actually changes.
    let stale = {
        let tab = app.tab();
        tab.cache
            .as_ref()
            .is_none_or(|c| c.rev != tab.content_rev || c.width != area.width)
    };
    if stale {
        let lines = {
            let tab = app.tab();
            app::transcript_lines(tab, area.width)
        };
        let tab = app.tab_mut();
        tab.cache = Some(app::Cache {
            rev: tab.content_rev,
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
    Paragraph::new(content).scroll((scroll, 0))
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

/// the powerline status bar.
pub fn status_bar(app: &App, width: u16) -> Line<'static> {
    const SEP: &str = "\u{e0b0}";
    let tab = app.tab();
    let short: String = tab.session.chars().take(8).collect();
    let mut segments: Vec<(String, Color, Color)> = vec![(
        format!(" {short} "),
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

    // the hint is contextual: while a turn runs, interrupting is the useful key.
    let busy = tab.status.turn_running;
    let hint_len = if busy {
        " esc interrupt \u{00b7} ctrl+t new \u{00b7} ctrl+w close "
            .chars()
            .count()
    } else {
        " ctrl+t new \u{00b7} ctrl+w close \u{00b7} ctrl+1..9 jump "
            .chars()
            .count()
    };
    let pad = usize::from(width).saturating_sub(used + hint_len);
    spans.push(Span::raw(" ".repeat(pad)));
    if busy {
        spans.push(Span::styled(
            " esc interrupt ",
            Style::default()
                .fg(Color::Rgb(0x10, 0x14, 0x1a))
                .bg(app::ACCENT)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            " ctrl+t new \u{00b7} ctrl+w close ",
            Style::default().fg(app::FAINT),
        ));
    } else {
        spans.push(Span::styled(
            " ctrl+t new \u{00b7} ctrl+w close \u{00b7} ctrl+1..9 jump ",
            Style::default().fg(app::FAINT),
        ));
    }
    Line::from(spans)
}

/// a bordered composer. returns where the text cursor belongs.
pub fn composer(frame: &mut Frame<'_>, area: Rect, app: &App) -> Option<(u16, u16)> {
    let tab = app.tab();
    let field = usize::from(area.width.saturating_sub(4));
    let (hscroll, column) = tab.input_view(field);
    frame.render_widget(
        Paragraph::new(tab.input_text().to_owned())
            .scroll((0, u16::try_from(hscroll).unwrap_or(u16::MAX)))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(RULE))
                    .padding(Padding::new(1, 1, 0, 0)),
            ),
        area,
    );
    Some((area.x + 2 + u16::try_from(column).unwrap_or(0), area.y + 1))
}

/// key/value facts about the session, for sidebars and dashboards.
pub fn facts(app: &App) -> Vec<(&'static str, String)> {
    let tab = app.tab();
    let mut out = vec![
        ("session", tab.session.chars().take(8).collect::<String>()),
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
