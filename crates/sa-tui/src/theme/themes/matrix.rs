//! digital rain aesthetic: dim green frame, bright glyph gutter, glyph prompt.

use crate::theme::prelude::*;

pub struct Theme;

const BRIGHT: Color = Color::Rgb(0x8f, 0xff, 0x8f);
const DIM: Color = Color::Rgb(0x1f, 0x5f, 0x2f);

impl crate::theme::Theme for Theme {
    fn name(&self) -> &'static str {
        "matrix"
    }

    fn blurb(&self) -> &'static str {
        "digital rain: dim green frame, glyph gutter, glyph prompt"
    }

    fn render(&self, frame: &mut Frame<'_>, app: &App) -> Option<(u16, u16)> {
        let area = frame.area();
        let frame_block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(DIM))
            .title(Line::from(Span::styled(
                " \u{30c6}\u{30fc}\u{30de} ",
                Style::default().fg(BRIGHT),
            )));
        let inner = frame_block.inner(area);
        frame.render_widget(frame_block, area);

        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(2), Constraint::Length(1)])
            .split(inner);

        let mut lines = Vec::new();
        for (index, line) in parts::transcript(app).into_iter().enumerate() {
            let glyph = if index % 3 == 0 {
                "\u{2591}"
            } else if index % 3 == 1 {
                "\u{2592}"
            } else {
                "\u{2593}"
            };
            let text: String = line.spans.iter().map(|s| s.content.to_string()).collect();
            lines.push(Line::from(vec![
                Span::styled(format!("{glyph} "), Style::default().fg(DIM)),
                Span::styled(text, Style::default().fg(BRIGHT)),
            ]));
        }
        frame.render_widget(
            Paragraph::new(lines)
                .wrap(Wrap { trim: false })
                .scroll((parts::scroll(app), 0)),
            rows[0],
        );

        let width = usize::from(rows[1].width.saturating_sub(3));
        let (scroll, column) = parts::input_view(app, width);
        frame.render_widget(
            Paragraph::new(format!("\u{25b6} {}", parts::input(app)))
                .style(Style::default().fg(BRIGHT))
                .scroll((0, u16::try_from(scroll).unwrap_or(u16::MAX))),
            rows[1],
        );
        Some((rows[1].x + 3 + u16::try_from(column).unwrap_or(0), rows[1].y))
    }
}
