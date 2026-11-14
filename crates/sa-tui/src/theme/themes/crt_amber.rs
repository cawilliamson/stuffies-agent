//! amber monochrome terminal with the prompt at the top and a rules line.

use crate::theme::prelude::*;

pub struct Theme;

const AMBER: Color = Color::Rgb(0xff, 0xb0, 0x00);

impl crate::theme::Theme for Theme {
    fn name(&self) -> &'static str {
        "crt-amber"
    }

    fn blurb(&self) -> &'static str {
        "amber monochrome, prompt above the transcript, ruled header"
    }

    fn render(&self, frame: &mut Frame<'_>, app: &App) -> Option<(u16, u16)> {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(2),
            ])
            .split(frame.area());

        frame.render_widget(
            Paragraph::new(format!(
                " {}   {}   {}",
                parts::session(app),
                parts::model(app),
                parts::rate(app)
            ))
            .style(Style::default().fg(AMBER)),
            rows[0],
        );
        frame.render_widget(
            Paragraph::new("\u{2500}".repeat(usize::from(rows[1].width)))
                .style(Style::default().fg(Color::Rgb(0x80, 0x58, 0x00))),
            rows[1],
        );

        let width = usize::from(rows[2].width.saturating_sub(2));
        let (scroll, column) = parts::input_view(app, width);
        frame.render_widget(
            Paragraph::new(format!(">{}", parts::input(app)))
                .style(Style::default().fg(AMBER))
                .scroll((0, u16::try_from(scroll).unwrap_or(u16::MAX))),
            rows[2],
        );

        frame.render_widget(
            Paragraph::new(parts::transcript(app))
                .wrap(Wrap { trim: false })
                .scroll((parts::scroll(app), 0))
                .style(Style::default().fg(Color::Rgb(0xd0, 0x90, 0x00))),
            rows[3],
        );
        Some((rows[2].x + 1 + u16::try_from(column).unwrap_or(0), rows[2].y))
    }
}
