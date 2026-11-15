//! heads-up display: corner brackets, facts floating in the corners.

use crate::theme::prelude::*;

pub struct Theme;

fn brackets(width: u16, height: u16) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    for y in 0..height {
        let mut row = String::new();
        for x in 0..width {
            let left = x < 2;
            let right = x + 2 >= width;
            let top = y < 2;
            let bottom = y + 2 >= height;
            row.push(if (top || bottom) && (left || right) {
                if top && left {
                    '\u{250c}'
                } else if top {
                    '\u{2510}'
                } else if left {
                    '\u{2514}'
                } else {
                    '\u{2518}'
                }
            } else if (top || bottom) && x < width - 2 {
                '\u{2500}'
            } else if (left || right) && y < height - 2 {
                '\u{2502}'
            } else {
                ' '
            });
        }
        out.push(Line::from(row));
    }
    out
}

impl crate::theme::Theme for Theme {
    fn name(&self) -> &'static str {
        "hud"
    }

    fn blurb(&self) -> &'static str {
        "corner brackets only, facts floating in the frame corners"
    }

    fn transcript_rows(&self, area: Rect) -> u16 {
        area.height.saturating_sub(6)
    }

    fn render(&self, frame: &mut Frame<'_>, app: &mut App) -> Option<(u16, u16)> {
        let area = frame.area();
        frame.render_widget(Paragraph::new(brackets(area.width, area.height)), area);

        let body = Rect {
            x: area.x + 3,
            y: area.y + 3,
            width: area.width.saturating_sub(6),
            height: area.height.saturating_sub(7),
        };
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(2), Constraint::Length(1)])
            .split(body);
        parts::transcript(frame, rows[0], app);

        let tab = app.tab();
        let field = usize::from(rows[1].width.saturating_sub(2));
        let (hscroll, column) = tab.input_view(field);
        frame.render_widget(
            Paragraph::new(format!("> {}", tab.input_text()))
                .scroll((0, u16::try_from(hscroll).unwrap_or(u16::MAX))),
            rows[1],
        );

        let head = parts::facts(app)
            .into_iter()
            .map(|(_, v)| v)
            .collect::<Vec<_>>()
            .join("  ");
        frame.render_widget(
            Paragraph::new(Span::styled(head, Style::default().fg(app::DIM))),
            Rect {
                x: area.x + 3,
                y: area.y + 1,
                width: area.width.saturating_sub(6),
                height: 1,
            },
        );
        Some((
            rows[1].x + 2 + u16::try_from(column).unwrap_or(0),
            rows[1].y,
        ))
    }
}
