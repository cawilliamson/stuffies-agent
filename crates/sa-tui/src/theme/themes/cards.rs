//! each exchange in its own card frame, composer styled as a card too.

use crate::theme::prelude::*;

pub struct Theme;

impl crate::theme::Theme for Theme {
    fn name(&self) -> &'static str {
        "cards"
    }

    fn blurb(&self) -> &'static str {
        "transcript split into card frames with a card composer"
    }

    fn render(&self, frame: &mut Frame<'_>, app: &App) -> Option<(u16, u16)> {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(3), Constraint::Length(4)])
            .split(frame.area());

        let mut lines: Vec<Line<'static>> = Vec::new();
        for (index, line) in parts::transcript(app).into_iter().enumerate() {
            let marker = if line.spans.first().map(|s| s.content.as_ref()) == Some("you │ ") {
                "\u{25c6}"
            } else if line.spans.first().map(|s| s.content.as_ref()) == Some("sa  │ ") {
                "\u{25cf}"
            } else {
                "\u{25cb}"
            };
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{marker} "),
                    Style::default().fg(if index % 2 == 0 { Color::Cyan } else { Color::DarkGray }),
                ),
                Span::styled(
                    line.spans.iter().map(|s| s.content.to_string()).collect::<String>(),
                    Style::default(),
                ),
            ]));
        }
        frame.render_widget(
            Paragraph::new(lines)
                .wrap(Wrap { trim: false })
                .scroll((parts::scroll(app), 0))
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_type(BorderType::Rounded)
                        .title(format!(" {} cards ", parts::session(app)))
                        .border_style(Style::default().fg(Color::Rgb(0x55, 0x55, 0x66))),
                ),
            rows[0],
        );

        let width = usize::from(rows[1].width.saturating_sub(4));
        let (scroll, column) = parts::input_view(app, width);
        frame.render_widget(
            Paragraph::new(format!("  {}", parts::input(app)))
                .scroll((0, u16::try_from(scroll).unwrap_or(u16::MAX)))
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_type(BorderType::Rounded)
                        .title(" compose ")
                        .border_style(Style::default().fg(Color::Cyan)),
                ),
            rows[1],
        );
        Some((rows[1].x + 3 + u16::try_from(column).unwrap_or(0), rows[1].y + 1))
    }
}
