//! three stacked bordered panels with numbered titles.

use crate::theme::prelude::*;

pub struct Theme;

impl crate::theme::Theme for Theme {
    fn name(&self) -> &'static str {
        "panels"
    }

    fn blurb(&self) -> &'static str {
        "stacked panels: transcript, prompt, and a metrics footer"
    }

    fn render(&self, frame: &mut Frame<'_>, app: &App) -> Option<(u16, u16)> {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(3),
                Constraint::Length(3),
                Constraint::Length(3),
            ])
            .split(frame.area());

        frame.render_widget(
            Paragraph::new(parts::transcript(app))
                .wrap(Wrap { trim: false })
                .scroll((parts::scroll(app), 0))
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" 1 transcript ")
                        .border_style(Style::default().fg(Color::Rgb(0x60, 0x60, 0x70))),
                ),
            rows[0],
        );

        let width = usize::from(rows[1].width.saturating_sub(2));
        let (scroll, column) = parts::input_view(app, width);
        frame.render_widget(
            Paragraph::new(parts::input(app).to_owned())
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" 2 prompt ")
                        .border_style(Style::default().fg(Color::Rgb(0x60, 0x60, 0x70))),
                )
                .scroll((0, u16::try_from(scroll).unwrap_or(u16::MAX))),
            rows[1],
        );

        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format!(" {} ", parts::session(app)), Style::default().fg(Color::Cyan)),
                Span::styled(format!(" {} ", parts::model(app)), Style::default().fg(Color::Blue)),
                Span::styled(format!(" {} ", parts::rate(app)), Style::default().fg(Color::Red)),
                Span::styled(format!(" {} ", parts::tokens(app)), Style::default().fg(Color::White)),
            ]))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" 3 metrics ")
                    .border_style(Style::default().fg(Color::Rgb(0x60, 0x60, 0x70))),
            ),
            rows[2],
        );
        Some((rows[1].x + 1 + u16::try_from(column).unwrap_or(0), rows[1].y + 1))
    }
}
