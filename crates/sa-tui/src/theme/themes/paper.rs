//! light page: dark ink on cream, ruled margin.

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

    fn transcript_rows(&self, area: Rect) -> u16 {
        area.height.saturating_sub(6)
    }

    fn render(&self, frame: &mut Frame<'_>, app: &mut App) -> Option<(u16, u16)> {
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
            Paragraph::new(parts::tab_bar(app, rows[0].width))
                .style(Style::default().fg(INK).bg(SHEET)),
            rows[0],
        );
        parts::transcript_with(frame, rows[1], app, |p| {
            p.style(Style::default().bg(SHEET).fg(INK))
        });

        let tab = app.tab();
        let field = usize::from(rows[2].width.saturating_sub(4));
        let (hscroll, column) = tab.input_view(field);
        frame.render_widget(
            Paragraph::new(tab.input_text().to_owned())
                .scroll((0, u16::try_from(hscroll).unwrap_or(u16::MAX)))
                .style(Style::default().fg(INK).bg(SHEET))
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(Color::Rgb(0xc0, 0xb8, 0xa0)))
                        .padding(Padding::new(1, 1, 0, 0)),
                ),
            rows[2],
        );
        Some((
            rows[2].x + 2 + u16::try_from(column).unwrap_or(0),
            rows[2].y + 1,
        ))
    }
}
