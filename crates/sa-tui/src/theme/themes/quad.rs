//! four walled quadrants.

use crate::theme::prelude::*;

pub struct Theme;

impl crate::theme::Theme for Theme {
    fn name(&self) -> &'static str {
        "quad"
    }

    fn blurb(&self) -> &'static str {
        "four walled quadrants: transcript, facts, prompt, keys"
    }

    fn transcript_rows(&self, area: Rect) -> u16 {
        (area.height * 6 / 10).saturating_sub(2)
    }

    fn render(&self, frame: &mut Frame<'_>, app: &mut App) -> Option<(u16, u16)> {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
            .split(frame.area());
        let top = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(70), Constraint::Percentage(30)])
            .split(rows[0]);
        let bottom = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
            .split(rows[1]);

        let border = parts::pane("transcript", Color::Rgb(0x44, 0x66, 0x88));
        let inner = border.inner(top[0]);
        frame.render_widget(border, top[0]);
        parts::transcript(frame, inner, app);

        let facts: Vec<Line<'static>> = parts::facts(app)
            .into_iter()
            .map(|(k, v)| Line::from(format!("{k}: {v}")))
            .collect();
        frame.render_widget(
            Paragraph::new(facts).block(parts::pane("facts", Color::Rgb(0x88, 0x66, 0x44))),
            top[1],
        );

        let border = parts::pane("prompt", Color::Rgb(0x44, 0x88, 0x66));
        let inner = border.inner(bottom[0]);
        frame.render_widget(border, bottom[0]);
        let cursor = parts::composer(frame, inner, app);

        frame.render_widget(
            Paragraph::new(vec![
                Line::from("ctrl+t  new tab"),
                Line::from("ctrl+w  close tab"),
                Line::from("esc     interrupt"),
                Line::from("/theme  reskin"),
            ])
            .block(parts::pane("keys", Color::Rgb(0x66, 0x44, 0x88))),
            bottom[1],
        );
        cursor
    }
}
