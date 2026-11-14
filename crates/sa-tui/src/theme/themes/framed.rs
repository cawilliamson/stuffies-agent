//! the whole app inside one thick window frame with a title bar.

use crate::theme::prelude::*;

pub struct Theme;

impl crate::theme::Theme for Theme {
    fn name(&self) -> &'static str {
        "framed"
    }

    fn blurb(&self) -> &'static str {
        "one heavy window frame around the transcript and prompt"
    }

    fn render(&self, frame: &mut Frame<'_>, app: &App) -> Option<(u16, u16)> {
        let outer = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Thick)
            .border_style(Style::default().fg(Color::Rgb(0x7a, 0x7a, 0x8a)))
            .title(Line::from(vec![
                Span::styled(" sa ", Style::default().fg(Color::Black).bg(Color::Rgb(0x7a, 0x7a, 0x8a))),
                Span::raw(" "),
                Span::styled(parts::session(app), Style::default().fg(Color::Rgb(0xd0, 0xd0, 0xe0))),
                Span::raw(" "),
                Span::styled(parts::rate(app), Style::default().fg(Color::Rgb(0xff, 0xa0, 0xa0))),
            ]));
        let inner = outer.inner(frame.area());
        frame.render_widget(outer, frame.area());

        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(3), Constraint::Length(3)])
            .split(inner);

        frame.render_widget(parts::transcript_paragraph(app), rows[0]);
        let width = usize::from(rows[1].width.saturating_sub(2));
        let (scroll, column) = parts::input_view(app, width);
        frame.render_widget(
            Paragraph::new(parts::input(app).to_owned())
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_type(BorderType::Rounded)
                        .title(" prompt ")
                        .border_style(Style::default().fg(Color::Rgb(0x7a, 0x7a, 0x8a))),
                )
                .scroll((0, u16::try_from(scroll).unwrap_or(u16::MAX))),
            rows[1],
        );
        Some((rows[1].x + 1 + u16::try_from(column).unwrap_or(0), rows[1].y + 1))
    }
}
