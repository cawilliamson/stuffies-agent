//! light theme: black on white, thin grey rules, like a printed page.

use crate::theme::prelude::*;

pub struct Theme;

const INK: Color = Color::Rgb(0x18, 0x18, 0x18);
const SHEET: Color = Color::Rgb(0xf4, 0xf1, 0xe8);

impl crate::theme::Theme for Theme {
    fn name(&self) -> &'static str {
        "paper"
    }

    fn blurb(&self) -> &'static str {
        "light page: dark ink on cream paper with a ruled margin"
    }

    fn render(&self, frame: &mut Frame<'_>, app: &App) -> Option<(u16, u16)> {
        frame.render_widget(
            Block::default().style(Style::default().bg(SHEET).fg(INK)),
            frame.area(),
        );
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(3),
                Constraint::Length(3),
            ])
            .split(frame.area());

        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(" page ", Style::default().fg(SHEET).bg(INK)),
                Span::styled(
                    format!(" {} · {} · {} ", parts::session(app), parts::model(app), parts::rate(app)),
                    Style::default().fg(INK).bg(SHEET),
                ),
            ])),
            rows[0],
        );

        let body: Vec<Line<'static>> = parts::transcript(app)
            .into_iter()
            .map(|line| {
                let text: String = line.spans.iter().map(|s| s.content.to_string()).collect();
                Line::from(vec![
                    Span::styled("\u{2502} ", Style::default().fg(Color::Rgb(0xc0, 0xb8, 0xa0))),
                    Span::styled(text, Style::default().fg(INK)),
                ])
            })
            .collect();
        frame.render_widget(
            Paragraph::new(body)
                .wrap(Wrap { trim: false })
                .scroll((parts::scroll(app), 0))
                .style(Style::default().bg(SHEET).fg(INK)),
            rows[1],
        );

        let width = usize::from(rows[2].width.saturating_sub(2));
        let (scroll, column) = parts::input_view(app, width);
        frame.render_widget(
            Paragraph::new(parts::input(app).to_owned())
                .scroll((0, u16::try_from(scroll).unwrap_or(u16::MAX)))
                .style(Style::default().bg(SHEET).fg(INK))
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(Color::Rgb(0xc0, 0xb8, 0xa0))),
                ),
            rows[2],
        );
        Some((rows[2].x + 1 + u16::try_from(column).unwrap_or(0), rows[2].y + 1))
    }
}
