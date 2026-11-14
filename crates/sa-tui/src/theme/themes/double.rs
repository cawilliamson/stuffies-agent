//! double-ruled boxes: transcript and prompt each in their own heavy frame.

use crate::theme::prelude::*;

pub struct Theme;

impl crate::theme::Theme for Theme {
    fn name(&self) -> &'static str {
        "double"
    }

    fn blurb(&self) -> &'static str {
        "double-ruled frames around transcript and prompt, header rule"
    }

    fn render(&self, frame: &mut Frame<'_>, app: &App) -> Option<(u16, u16)> {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(3),
                Constraint::Length(3),
            ])
            .split(frame.area());

        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(" SESSION ", Style::default().fg(Color::Black).bg(Color::White)),
                Span::raw(format!(" {}  ", parts::session(app))),
                Span::styled(" RATE ", Style::default().fg(Color::Black).bg(Color::Red)),
                Span::raw(format!(" {}  ", parts::rate(app))),
                Span::styled(" TOKENS ", Style::default().fg(Color::Black).bg(Color::DarkGray)),
                Span::raw(format!(" {} ", parts::tokens(app))),
            ]))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Double),
            ),
            rows[0],
        );

        frame.render_widget(
            Paragraph::new(parts::transcript(app))
                .wrap(Wrap { trim: false })
                .scroll((parts::scroll(app), 0))
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_type(BorderType::Double)
                        .title(" transcript "),
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
                        .border_type(BorderType::Double)
                        .title(" prompt "),
                ),
            rows[2],
        );
        Some((rows[2].x + 1 + u16::try_from(column).unwrap_or(0), rows[2].y + 1))
    }
}
