//! dashboard footer of boxed metric cells under the transcript.

use crate::theme::prelude::*;

pub struct Theme;

fn cell(title: &str, value: String, accent: Color) -> Paragraph<'static> {
    Paragraph::new(Line::from(Span::styled(value, Style::default().fg(accent))))
        .block(parts::pane(title, parts::RULE))
}

impl crate::theme::Theme for Theme {
    fn name(&self) -> &'static str {
        "footer-grid"
    }

    fn blurb(&self) -> &'static str {
        "dashboard footer of boxed metric cells below the transcript"
    }

    fn transcript_rows(&self, area: Rect) -> u16 {
        area.height.saturating_sub(9)
    }

    fn render(&self, frame: &mut Frame<'_>, app: &mut App) -> Option<(u16, u16)> {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(3),
                Constraint::Length(4),
                Constraint::Length(3),
            ])
            .split(frame.area());

        frame.render_widget(Paragraph::new(parts::tab_bar(app, rows[0].width)), rows[0]);
        parts::transcript(frame, rows[1], app);

        let grid = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Ratio(1, 4),
                Constraint::Ratio(1, 4),
                Constraint::Ratio(1, 4),
                Constraint::Ratio(1, 4),
            ])
            .split(rows[2]);
        let facts = parts::facts(app);
        let value = |key: &str| {
            facts
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v.clone())
                .unwrap_or_default()
        };
        frame.render_widget(cell("cwd", value("cwd"), app::ACCENT), grid[0]);
        frame.render_widget(cell("model", value("model"), app::OK), grid[1]);
        frame.render_widget(cell("rate", value("rate"), app::BAD), grid[2]);
        frame.render_widget(cell("tokens", value("tokens"), app::INK), grid[3]);
        parts::composer(frame, rows[3], app)
    }
}
