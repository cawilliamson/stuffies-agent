//! heads-up display: corner brackets, no boxes, facts in the corners.

use crate::theme::prelude::*;

pub struct Theme;

fn corner(width: u16, height: u16) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for y in 0..height {
        let mut row = String::new();
        for x in 0..width {
            let left = x < 2;
            let right = x + 2 >= width;
            let top = y < 2;
            let bottom = y + 2 >= height;
            let ch = if (top || bottom) && (left || right) {
                if top && left {
                    '\u{250c}'
                } else if top && right {
                    '\u{2510}'
                } else if bottom && left {
                    '\u{2514}'
                } else {
                    '\u{2518}'
                }
            } else if (top || bottom) && (x < width - 2) {
                '\u{2500}'
            } else if (left || right) && y < height - 2 {
                '\u{2502}'
            } else {
                ' '
            };
            row.push(ch);
        }
        lines.push(Line::from(row));
    }
    lines
}

impl crate::theme::Theme for Theme {
    fn name(&self) -> &'static str {
        "hud"
    }

    fn blurb(&self) -> &'static str {
        "corner brackets only, facts floating in the frame corners"
    }

    fn render(&self, frame: &mut Frame<'_>, app: &App) -> Option<(u16, u16)> {
        let area = frame.area();
        frame.render_widget(Paragraph::new(corner(area.width, area.height)), area);

        let body = Rect {
            x: area.x + 3,
            y: area.y + 2,
            width: area.width.saturating_sub(6),
            height: area.height.saturating_sub(5),
        };
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(2), Constraint::Length(1)])
            .split(body);
        frame.render_widget(parts::transcript_paragraph(app), rows[0]);

        let width = usize::from(rows[1].width.saturating_sub(2));
        let (scroll, column) = parts::input_view(app, width);
        frame.render_widget(
            Paragraph::new(format!("> {}", parts::input(app)))
                .scroll((0, u16::try_from(scroll).unwrap_or(u16::MAX))),
            rows[1],
        );

        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format!(" {} ", parts::session(app)), Style::default().fg(Color::Black).bg(Color::Cyan)),
                Span::styled(format!(" {} ", parts::rate(app)), Style::default().fg(Color::Black).bg(Color::Red)),
            ])),
            Rect { x: area.x + 2, y: area.y, width: area.width.saturating_sub(4), height: 1 },
        );
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(" {} {} ", parts::model(app), parts::tokens(app)),
                Style::default().fg(Color::DarkGray),
            )))
            .alignment(Alignment::Right),
            Rect { x: area.x + 2, y: area.y + area.height.saturating_sub(1), width: area.width.saturating_sub(4), height: 1 },
        );

        Some((rows[1].x + 2 + u16::try_from(column).unwrap_or(0), rows[1].y))
    }
}
