//! drafting table: light ink on blue with a ruled header.

use crate::theme::prelude::*;

pub struct Theme;

const INK: Color = Color::Rgb(0xdc, 0xe8, 0xff);
const PAPER: Color = Color::Rgb(0x10, 0x28, 0x50);

impl crate::theme::Theme for Theme {
    fn name(&self) -> &'static str {
        "blueprint"
    }

    fn blurb(&self) -> &'static str {
        "drafting-table blueprint with ruled header and spec frame"
    }

    fn transcript_rows(&self, area: Rect) -> u16 {
        area.height.saturating_sub(7)
    }

    fn render(&self, frame: &mut Frame<'_>, app: &mut App) -> Option<(u16, u16)> {
        frame.render_widget(
            Block::default().style(Style::default().bg(PAPER)),
            frame.area(),
        );
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(3),
                Constraint::Length(3),
                Constraint::Length(1),
            ])
            .split(frame.area());

        let rule: String = (0..rows[0].width)
            .map(|x| if x % 10 == 0 { '+' } else { '-' })
            .collect();
        frame.render_widget(
            Paragraph::new(rule).style(Style::default().fg(INK).bg(PAPER)),
            rows[0],
        );

        let inner = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(INK))
            .title(" sheet 1 ")
            .inner(rows[1]);
        frame.render_widget(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(INK))
                .title(" sheet 1 ")
                .style(Style::default().bg(PAPER)),
            rows[1],
        );
        parts::transcript(frame, inner, app);

        let cursor = parts::composer(frame, rows[2], app);
        frame.render_widget(
            Paragraph::new(parts::status_bar(app, rows[3].width)).style(Style::default().bg(PAPER)),
            rows[3],
        );
        cursor
    }
}
