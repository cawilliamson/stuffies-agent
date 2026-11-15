//! transcript with a left timeline gutter of markers.

use crate::theme::prelude::*;

pub struct Theme;

impl crate::theme::Theme for Theme {
    fn name(&self) -> &'static str {
        "timeline"
    }

    fn blurb(&self) -> &'static str {
        "left timeline gutter of markers beside the transcript"
    }

    fn transcript_rows(&self, area: Rect) -> u16 {
        area.height.saturating_sub(6)
    }

    fn render(&self, frame: &mut Frame<'_>, app: &mut App) -> Option<(u16, u16)> {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(3), Constraint::Min(20)])
            .split(frame.area());
        let right = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(3),
                Constraint::Length(3),
            ])
            .split(cols[1]);

        let gutter: Vec<Line<'static>> = (0..cols[0].height)
            .map(|y| {
                if y % 4 == 0 {
                    Line::from(Span::styled(
                        "\u{25cf}",
                        Style::default().fg(Color::Rgb(0x66, 0xcc, 0xff)),
                    ))
                } else {
                    Line::from(Span::styled(
                        "\u{2502}",
                        Style::default().fg(Color::Rgb(0x33, 0x44, 0x55)),
                    ))
                }
            })
            .collect();
        frame.render_widget(Paragraph::new(gutter), cols[0]);

        frame.render_widget(Paragraph::new(parts::tab_bar(app, right[0].width)), right[0]);
        frame.render_widget(parts::transcript(app, right[1]), right[1]);
        parts::composer(frame, right[2], app)
    }
}
