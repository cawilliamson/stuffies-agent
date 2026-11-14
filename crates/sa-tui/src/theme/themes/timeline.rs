//! transcript with a left timeline gutter of markers and connecting rules.

use crate::theme::prelude::*;

pub struct Theme;

impl crate::theme::Theme for Theme {
    fn name(&self) -> &'static str {
        "timeline"
    }

    fn blurb(&self) -> &'static str {
        "left timeline gutter with markers and rules beside the transcript"
    }

    fn render(&self, frame: &mut Frame<'_>, app: &App) -> Option<(u16, u16)> {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(3), Constraint::Min(20)])
            .split(frame.area());
        let right = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(3), Constraint::Length(3)])
            .split(cols[1]);

        let lines = parts::transcript(app);
        let gutter: Vec<Line<'static>> = (0..lines.len())
            .map(|index| {
                if index % 4 == 0 {
                    Line::from(Span::styled(
                        "\u{25cf}",
                        Style::default().fg(Color::Rgb(0x66, 0xcc, 0xff)),
                    ))
                } else {
                    Line::from(Span::styled("\u{2502}", Style::default().fg(Color::Rgb(0x33, 0x44, 0x55))))
                }
            })
            .collect();
        frame.render_widget(
            Paragraph::new(gutter).scroll((parts::scroll(app), 0)),
            cols[0],
        );

        frame.render_widget(
            Paragraph::new(lines)
                .wrap(Wrap { trim: false })
                .scroll((parts::scroll(app), 0)),
            right[0],
        );

        let width = usize::from(right[1].width.saturating_sub(2));
        let (scroll, column) = parts::input_view(app, width);
        frame.render_widget(
            Paragraph::new(parts::input(app).to_owned())
                .scroll((0, u16::try_from(scroll).unwrap_or(u16::MAX)))
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(format!(" {} · {} ", parts::session(app), parts::rate(app)))
                        .border_style(Style::default().fg(Color::Rgb(0x33, 0x44, 0x55))),
                ),
            right[1],
        );
        Some((right[1].x + 1 + u16::try_from(column).unwrap_or(0), right[1].y + 1))
    }
}
