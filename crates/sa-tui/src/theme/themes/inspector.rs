//! transcript with a docked tool-log strip above a composer bar.

use crate::theme::prelude::*;

pub struct Theme;

impl crate::theme::Theme for Theme {
    fn name(&self) -> &'static str {
        "inspector"
    }

    fn blurb(&self) -> &'static str {
        "docked tool-log strip and a composer bar under the transcript"
    }

    fn render(&self, frame: &mut Frame<'_>, app: &App) -> Option<(u16, u16)> {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(3),
                Constraint::Length(6),
                Constraint::Length(3),
            ])
            .split(frame.area());

        frame.render_widget(
            Paragraph::new(parts::transcript(app))
                .wrap(Wrap { trim: false })
                .scroll((parts::scroll(app), 0))
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" conversation ")
                        .border_style(Style::default().fg(Color::Rgb(0x50, 0x50, 0x60))),
                ),
            rows[0],
        );

        let tail: Vec<Line<'static>> = parts::transcript(app)
            .into_iter()
            .rev()
            .take(5)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        frame.render_widget(
            Paragraph::new(tail).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" live tail ")
                    .border_style(Style::default().fg(Color::Rgb(0x50, 0x50, 0x60))),
            ),
            rows[1],
        );

        let width = usize::from(rows[2].width.saturating_sub(2));
        let (scroll, column) = parts::input_view(app, width);
        frame.render_widget(
            Paragraph::new(parts::input(app).to_owned())
                .scroll((0, u16::try_from(scroll).unwrap_or(u16::MAX)))
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(format!(" {} · {} ", parts::session(app), parts::rate(app)))
                        .border_style(Style::default().fg(Color::Rgb(0x50, 0x50, 0x60))),
                ),
            rows[2],
        );
        Some((rows[2].x + 1 + u16::try_from(column).unwrap_or(0), rows[2].y + 1))
    }
}
