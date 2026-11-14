//! transcript on the left, a wide right inspector with a key/value table.

use crate::theme::prelude::*;

pub struct Theme;

fn rows(app: &App) -> Vec<Line<'static>> {
    let mut rows = vec![
        Line::from(vec![
            Span::styled("session  ", Style::default().fg(Color::DarkGray)),
            Span::styled(parts::session(app), Style::default().fg(Color::Cyan)),
        ]),
        Line::from(vec![
            Span::styled("state    ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                if parts::busy(app) { "working" } else { "ready" },
                Style::default().fg(if parts::busy(app) { Color::Yellow } else { Color::Green }),
            ),
        ]),
    ];
    if let Some(branch) = parts::branch(app) {
        rows.push(Line::from(vec![
            Span::styled("branch   ", Style::default().fg(Color::DarkGray)),
            Span::styled(branch, Style::default().fg(Color::Magenta)),
        ]));
    }
    rows.push(Line::from(vec![
        Span::styled("model    ", Style::default().fg(Color::DarkGray)),
        Span::styled(parts::model(app), Style::default().fg(Color::Blue)),
    ]));
    rows.push(Line::from(vec![
        Span::styled("rate     ", Style::default().fg(Color::DarkGray)),
        Span::styled(parts::rate(app), Style::default().fg(Color::Red)),
    ]));
    rows.push(Line::from(vec![
        Span::styled("tokens   ", Style::default().fg(Color::DarkGray)),
        Span::styled(parts::tokens(app), Style::default().fg(Color::White)),
    ]));
    rows
}

impl crate::theme::Theme for Theme {
    fn name(&self) -> &'static str {
        "sidebar-right"
    }

    fn blurb(&self) -> &'static str {
        "transcript left, key/value inspector rail on the right"
    }

    fn render(&self, frame: &mut Frame<'_>, app: &App) -> Option<(u16, u16)> {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(20), Constraint::Length(30)])
            .split(frame.area());
        let left = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(3), Constraint::Length(3)])
            .split(cols[0]);

        frame.render_widget(parts::transcript_paragraph(app), left[0]);
        let width = usize::from(left[1].width.saturating_sub(2));
        let (scroll, column) = parts::input_view(app, width);
        frame.render_widget(
            Paragraph::new(parts::input(app).to_owned())
                .block(Block::default().borders(Borders::ALL).title(" prompt "))
                .scroll((0, u16::try_from(scroll).unwrap_or(u16::MAX))),
            left[1],
        );

        frame.render_widget(
            Paragraph::new(rows(app)).block(
                Block::default()
                    .borders(Borders::LEFT)
                    .title(" inspector ")
                    .border_style(Style::default().fg(Color::DarkGray)),
            ),
            cols[1],
        );
        Some((left[1].x + 1 + u16::try_from(column).unwrap_or(0), left[1].y + 1))
    }
}
