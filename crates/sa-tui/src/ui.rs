//! the one screen layout, and the pieces it is built from. there is no theming layer:
//! adding a second visual means adding a second `render`, not a registry.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, Padding, Paragraph, Scrollbar, ScrollbarOrientation,
    ScrollbarState,
};
use ratatui::Frame;

use crate::app::{self, App};

/// the tallest the composer may grow to hold a multi-line prompt.
const MAX_COMPOSER_ROWS: usize = 10;

/// the muted rule colour used across layouts.
pub const RULE: Color = Color::Rgb(0x2a, 0x30, 0x3a);

/// the transcript for a pane, pinned to the newest content when following.
pub fn transcript(frame: &mut Frame<'_>, area: Rect, app: &mut App) {
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
    frame.render_widget(Paragraph::new(content).scroll((scroll, 0)), area);

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

/// the busy marker on a tab: a star turning through its points, the way macos draws work
/// in progress, rather than a solid glyph blinking on and off.
const BUSY_FRAMES: [&str; 6] = [
    "\u{2736}", "\u{2738}", "\u{2739}", "\u{273a}", "\u{2739}", "\u{2738}",
];

/// shortens a tab label while keeping its trailing `(session)` marker intact, then cuts it
/// hard so the result never exceeds the budget.
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
    let fitted = format!("{head}\u{2026}{tail}");
    if fitted.chars().count() <= max {
        return fitted;
    }
    // the marker alone can be wider than the budget, so cut to what is really there.
    let mut cut: String = fitted.chars().take(max.saturating_sub(1)).collect();
    if max > 0 {
        cut.push('\u{2026}');
    }
    cut
}

/// the widest a single tab label may be, before it is cut to keep its id marker.
const TAB_LABEL_MAX: usize = 28;

/// one tab's text: the busy spinner, its position, and its title.
fn tab_text(app: &App, index: usize, cap: usize) -> String {
    let tab = &app.tabs[index];
    let mark = if tab.status.turn_running {
        format!("{} ", BUSY_FRAMES[app.spinner % BUSY_FRAMES.len()])
    } else {
        String::new()
    };
    fit_label(&format!("{mark}{} {}", index + 1, tab.title), cap)
}

/// how wide a label may be, so the active tab always fits beside its markers.
fn label_cap(room: usize) -> usize {
    TAB_LABEL_MAX.min(room.saturating_sub(6).max(8))
}

/// the cells one tab occupies: its chip padding and its text, before any divider.
fn tab_cells(label: &str) -> usize {
    label.chars().count() + 2
}

/// the cells a run of tabs needs, including the divider between each and the marker on
/// whichever end still has tabs hidden behind it.
fn run_cells(widths: &[usize], start: usize, end: usize) -> usize {
    let tabs: usize = widths[start..end].iter().sum::<usize>() + (end - start).saturating_sub(1);
    tabs + usize::from(start > 0) + usize::from(end < widths.len())
}

/// the run of tabs to draw: as many as fit, positioned so the active tab is on screen and
/// the strip is flush with whichever end it has reached. that is what stops the active tab
/// drifting into the middle with a dead margin beside it.
fn visible_tabs(widths: &[usize], active: usize, room: usize) -> (usize, usize) {
    let len = widths.len();
    if len == 0 {
        return (0, 0);
    }
    let active = active.min(len - 1);

    for count in (1..=len).rev() {
        // the window must contain the active tab, and cannot run off either end.
        let lowest = active.saturating_sub(count - 1);
        let highest = active.min(len - count);
        // centre the active tab, then clamp: near either end the strip sits against it
        // rather than leaving the space it would have centred into.
        let start = active
            .saturating_sub((count - 1) / 2)
            .clamp(lowest, highest);
        if run_cells(widths, start, start + count) <= room {
            return (start, start + count);
        }
    }
    // a single tab that alone overflows: show it anyway, clipped.
    (active, active + 1)
}

/// the cells the working label occupies on the composer's top edge. the activity run starts
/// after it rather than sweeping through the word.
fn working_label_cells(tab: &app::Tab) -> u16 {
    if tab.tool_running {
        return 0;
    }
    tab.working.as_ref().map_or(0, |word| {
        u16::try_from(word.chars().count() + 3).unwrap_or(0)
    })
}

/// an indeterminate run sliding along the composer's top edge. it says that work is
/// happening without pretending to know how much is left.
fn activity_bar(frame: &mut Frame<'_>, area: Rect, app: &App, skip: u16) {
    let reserved = usize::from(skip);
    if area.height == 0 || usize::from(area.width) < reserved + 10 {
        return;
    }
    let track = usize::from(area.width).saturating_sub(2 + reserved);
    let run = (track / 4).clamp(4, 16);
    let travel = track + run;
    let offset = i64::try_from((app.spinner * 3) % travel).unwrap_or_default();
    let run_length = i64::try_from(run).unwrap_or_default();
    let track_length = i64::try_from(track).unwrap_or_default();
    let buffer = frame.buffer_mut();
    for step in 0..run {
        let position = offset - run_length + i64::try_from(step).unwrap_or_default();
        if position < 0 || position >= track_length {
            continue;
        }
        let x = area.x + 1 + skip + u16::try_from(position).unwrap_or_default();
        if let Some(cell) = buffer.cell_mut((x, area.y)) {
            cell.set_char('\u{2500}')
                .set_style(Style::default().fg(app::ACCENT));
        }
    }
}

/// lists the choices that would complete the argument being typed.
fn suggestion_list(frame: &mut Frame<'_>, above: Rect, title: &str, items: Vec<String>) {
    if items.is_empty() {
        return;
    }
    let widest = items
        .iter()
        .map(|item| item.chars().count())
        .max()
        .unwrap_or_default();
    let width = u16::try_from(widest + 4)
        .unwrap_or(above.width)
        .clamp(16, above.width);
    let height = u16::try_from(items.len()).unwrap_or(1) + 2;
    let rect = Rect {
        x: above.x,
        y: above.y.saturating_sub(height),
        width,
        height,
    };

    frame.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(RULE))
        .title(Line::from(format!(" {title} ")).style(Style::default().fg(app::DIM)));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let lines: Vec<Line<'static>> = items
        .into_iter()
        .map(|item| Line::from(Span::styled(item, Style::default().fg(app::INK))))
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

/// the session tab strip. it slides to keep the active tab visible and marks that tabs are
/// hidden on either side, the way a browser does.
#[must_use]
pub fn tab_bar(app: &App, width: u16) -> Line<'static> {
    let room = usize::from(width);
    let cap = label_cap(room);
    let labels: Vec<String> = (0..app.tabs.len())
        .map(|index| tab_text(app, index, cap))
        .collect();
    let widths: Vec<usize> = labels.iter().map(|label| tab_cells(label)).collect();
    let (start, end) = visible_tabs(&widths, app.active, room);

    let mut spans: Vec<Span<'static>> = Vec::new();
    if start > 0 {
        spans.push(Span::styled("\u{2039}", Style::default().fg(app::FAINT)));
    }
    for (offset, label) in labels[start..end].iter().enumerate() {
        if offset > 0 {
            spans.push(Span::styled("\u{2502}", Style::default().fg(app::FAINT)));
        }
        // the chip against a marker loses the padding the marker already provides, so the
        // strip reads `\u{2039}12 ... 18\u{203a}` rather than `\u{2039} 12 ... 18 \u{203a}`.
        let first = offset == 0 && start > 0;
        let last = start + offset + 1 == end && end < labels.len();
        let chip = match (first, last) {
            (true, true) => label.clone(),
            (true, false) => format!("{label} "),
            (false, true) => format!(" {label}"),
            (false, false) => format!(" {label} "),
        };
        if start + offset == app.active {
            spans.push(Span::styled(
                chip,
                Style::default()
                    .fg(Color::Rgb(0x10, 0x14, 0x1a))
                    .bg(app::ACCENT)
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            spans.push(Span::styled(chip, Style::default().fg(app::DIM)));
        }
    }
    if end < labels.len() {
        spans.push(Span::styled("\u{203a}", Style::default().fg(app::FAINT)));
    }

    Line::from(spans)
}

/// a compact context meter: filled cells and a percentage, coloured by pressure.
fn context_meter(used: u64, limit: u64) -> (String, Color) {
    const CELLS: usize = 10;
    // no advertised window: show what was measured and invent no percentage.
    if limit == 0 {
        return (format!(" {} tokens", compact(used)), app::DIM);
    }
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
    // one space between the bar and the figure, no padding. below one percent the share is
    // shown to a tenth, because a real conversation on a million-token window sits at a
    // whole zero for a long time and a figure that never moves reads as broken.
    let share = if pct >= 1 {
        format!("{pct}%")
    } else {
        format!("{:.1}%", used as f64 * 100.0 / limit as f64)
    };
    (
        format!(
            "{}{} {share}",
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
    if let Some(branch) = &tab.branch {
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
    let (meter, ink) = context_meter(tab.status.context_tokens, app.context_limit(tab));
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
    let (lines, cursor_line, cursor_column) = tab.input_lines();
    let inner_height = usize::from(area.height.saturating_sub(2)).max(1);
    let field = usize::from(area.width.saturating_sub(4)).max(1);
    // keep the cursor in view: along its own line, and vertically within the box.
    let hscroll = cursor_column.saturating_sub(field.saturating_sub(1));
    let vscroll = cursor_line.saturating_sub(inner_height.saturating_sub(1));

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(RULE))
        .padding(Padding::new(1, 1, 0, 0));
    let mut block = block;

    // what the model is doing, on the edge the operator is already looking at. the moving
    // bar below says it is alive; this says what it is.
    if tab.status.turn_running && !tab.tool_running {
        if let Some(word) = &tab.working {
            block = block.title(
                Line::from(format!(" {word}\u{2026} ")).style(Style::default().fg(app::ACCENT)),
            );
        }
    }

    let mut rendered: Vec<Line<'static>> = lines
        .iter()
        .map(|line| Line::from(Span::styled(line.clone(), Style::default().fg(app::INK))))
        .collect();
    // the rest of the completion, shown where it would land.
    if let Some(ghost) = tab.suggestion() {
        if let Some(last) = rendered.last_mut() {
            last.push_span(Span::styled(ghost, Style::default().fg(app::FAINT)));
        }
    }

    frame.render_widget(
        Paragraph::new(rendered)
            .scroll((
                u16::try_from(vscroll).unwrap_or(u16::MAX),
                u16::try_from(hscroll).unwrap_or(u16::MAX),
            ))
            .block(block),
        area,
    );
    Some((
        area.x + 2 + u16::try_from(cursor_column.saturating_sub(hscroll)).unwrap_or(0),
        area.y + 1 + u16::try_from(cursor_line.saturating_sub(vscroll)).unwrap_or(0),
    ))
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

/// draws the open picker over the interface.
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

/// draws the yes/no question over the interface.
pub fn confirm_prompt(frame: &mut Frame<'_>, app: &App) {
    let Some(question) = &app.confirm else {
        return;
    };
    let screen = frame.area();
    let width = screen
        .width
        .saturating_sub(8)
        .clamp(24, 60)
        .min(screen.width);
    let height = 4.min(screen.height);
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
        .title(Line::from(" close tab ").style(Style::default().fg(app::ACCENT)));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let lines = vec![
        Line::from(Span::styled(
            question.clone(),
            Style::default().fg(app::INK),
        )),
        Line::from(Span::styled(
            "y close \u{00b7} n keep",
            Style::default().fg(app::FAINT),
        )),
    ];
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

/// draws the whole screen, returning where the text cursor belongs.
pub fn render(frame: &mut Frame<'_>, app: &mut App) -> Option<(u16, u16)> {
    // a composed prompt may be several lines: give it the room, up to a cap.
    let input_rows = app.tab().input_lines().0.len().clamp(1, MAX_COMPOSER_ROWS);
    let composer_height = u16::try_from(input_rows).unwrap_or(1).saturating_add(2);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(composer_height),
            Constraint::Length(1),
        ])
        .split(frame.area());

    frame.render_widget(Paragraph::new(tab_bar(app, rows[0].width)), rows[0]);

    let border = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(RULE));
    let inner = border.inner(rows[1]);
    frame.render_widget(border, rows[1]);
    transcript(frame, inner, app);

    let cursor = composer(frame, rows[2], app);
    // work in progress reads as a moving bar along the composer's edge, the way a browser
    // shows an indeterminate load, rather than as a character spinning in place.
    if app.tab().status.turn_running {
        activity_bar(frame, rows[2], app, working_label_cells(app.tab()));
    }
    // the choices for the argument being typed sit directly above the composer, where the
    // operator is looking.
    suggestion_list(frame, rows[2], "cwd", app.tab().cwd_suggestions());
    let levels = app.effort_levels(app.tab());
    suggestion_list(
        frame,
        rows[2],
        "effort",
        app.tab().effort_suggestions(&levels),
    );
    frame.render_widget(Paragraph::new(status_bar(app, rows[3].width)), rows[3]);
    cursor
}
