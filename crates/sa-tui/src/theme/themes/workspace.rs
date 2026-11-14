//! an IDE-shaped workspace: labelled project rail, transcript, docked composer.

use crate::theme::prelude::*;

pub struct Theme;

const PANEL: Color = Color::Rgb(0x23, 0x27, 0x2e);
const RULE: Color = Color::Rgb(0x2e, 0x33, 0x3c);
const MUTED: Color = Color::Rgb(0x6c, 0x76, 0x86);
const ACCENT: Color = Color::Rgb(0x8a, 0xc8, 0xff);

fn rail(app: &App) -> Vec<Line<'static>> {
    let mut rows = vec![
        Line::from(Span::styled("  PROJECT", Style::default().fg(MUTED).add_modifier(Modifier::BOLD))),
        Line::from(""),
    ];
    let mut fact = |icon: &'static str, label: &str, value: String| {
        // keep the rail tidy: long values are shortened rather than clipped mid-glyph.
        let mut value = value;
        if value.chars().count() > 18 {
            value = format!("{}\u{2026}", value.chars().take(17).collect::<String>());
        }
        rows.push(Line::from(vec![
            Span::styled(format!("  {icon} "), Style::default().fg(RULE)),
            Span::styled(format!("{label:<9}"), Style::default().fg(MUTED)),
            Span::styled(value, Style::default().fg(Color::Rgb(0xc8, 0xd0, 0xdc))),
        ]));
    };
    fact("\u{f120}", "session", parts::session(app));
    if let Some(branch) = parts::branch(app) {
        fact("\u{e725}", "branch", branch);
    }
    fact("\u{f2db}", "model", parts::model(app));
    fact("\u{f0e4}", "rate", parts::rate(app));
    fact("\u{f0e7}", "tokens", parts::tokens(app));

    rows.push(Line::from(""));
    rows.push(Line::from(Span::styled("  KEYS", Style::default().fg(MUTED).add_modifier(Modifier::BOLD))));
    rows.push(Line::from(""));
    for (key, action) in [
        ("enter", "send"),
        ("esc", "interrupt"),
        ("ctrl-c", "quit"),
        ("/theme", "reskin"),
    ] {
        rows.push(Line::from(vec![
            Span::styled(format!("  {key:<8}"), Style::default().fg(RULE)),
            Span::styled(action.to_owned(), Style::default().fg(MUTED)),
        ]));
    }
    rows
}

impl crate::theme::Theme for Theme {
    fn name(&self) -> &'static str {
        "workspace"
    }

    fn blurb(&self) -> &'static str {
        "IDE-shaped workspace with a labelled project rail and docked composer"
    }

    fn transcript_rows(&self, area: Rect) -> u16 {
        area.height.saturating_sub(5)
    }

    fn render(&self, frame: &mut Frame<'_>, app: &App) -> Option<(u16, u16)> {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(34), Constraint::Min(20)])
            .split(frame.area());
        let main = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(3), Constraint::Length(3)])
            .split(cols[1]);

        frame.render_widget(
            Paragraph::new(rail(app))
                .style(Style::default().bg(PANEL))
                .block(
                    Block::default()
                        .borders(Borders::RIGHT)
                        .border_style(Style::default().fg(RULE)),
                ),
            cols[0],
        );

        frame.render_widget(
            Paragraph::new(parts::messages(app, &parts::Ink::default(), main[0].width))
                .wrap(Wrap { trim: false })
                .scroll((parts::scroll(app), 0)),
            main[0],
        );

        let width = usize::from(main[1].width.saturating_sub(4));
        let (scroll, column) = parts::input_view(app, width);
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("\u{276f} ", Style::default().fg(ACCENT)),
                Span::raw(parts::input(app).to_owned()),
            ]))
            .scroll((0, u16::try_from(scroll).unwrap_or(u16::MAX)))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(RULE))
                    .title(Span::styled(" message ", Style::default().fg(MUTED)))
                    .padding(ratatui::widgets::Padding::new(1, 1, 0, 0)),
            ),
            main[1],
        );

        Some((
            main[1].x + 3 + u16::try_from(column).unwrap_or(0),
            main[1].y + 1,
        ))
    }
}
