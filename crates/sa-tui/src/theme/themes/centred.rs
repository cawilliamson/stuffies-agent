//! a narrow centred column with generous margins, like a document.

use crate::theme::prelude::*;

pub struct Theme;

impl crate::theme::Theme for Theme {
    fn name(&self) -> &'static str {
        "centred"
    }

    fn blurb(&self) -> &'static str {
        "narrow centred document column with wide margins"
    }

    fn render(&self, frame: &mut Frame<'_>, app: &App) -> Option<(u16, u16)> {
        let area = frame.area();
        let width = area.width.min(76);
        let margin = (area.width - width) / 2;
        let column = Rect {
            x: area.x + margin,
            y: area.y,
            width,
            height: area.height,
        };

        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(3),
                Constraint::Length(3),
            ])
            .split(column);

        frame.render_widget(
            Paragraph::new(Span::styled(
                parts::session(app),
                Style::default().fg(Color::DarkGray),
            ))
            .alignment(Alignment::Center),
            rows[0],
        );
        frame.render_widget(parts::transcript_paragraph(app), rows[1]);

        let inner = usize::from(rows[2].width.saturating_sub(2));
        let (scroll, column_x) = parts::input_view(app, inner);
        frame.render_widget(
            Paragraph::new(parts::input(app).to_owned())
                .scroll((0, u16::try_from(scroll).unwrap_or(u16::MAX)))
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_type(BorderType::Rounded)
                        .border_style(Style::default().fg(Color::Rgb(0x60, 0x60, 0x60))),
                ),
            rows[2],
        );
        Some((rows[2].x + 1 + u16::try_from(column_x).unwrap_or(0), rows[2].y + 1))
    }
}
