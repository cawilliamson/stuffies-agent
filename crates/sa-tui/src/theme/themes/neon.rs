//! cyberpunk: magenta and cyan chevrons, glowing frame, slash prompt.

use crate::theme::prelude::*;

pub struct Theme;

const MAGENTA: Color = Color::Rgb(0xff, 0x2d, 0x95);
const CYAN: Color = Color::Rgb(0x00, 0xff, 0xe1);

impl crate::theme::Theme for Theme {
    fn name(&self) -> &'static str {
        "neon"
    }

    fn blurb(&self) -> &'static str {
        "cyberpunk magenta/cyan chevrons with a glowing frame"
    }

    fn render(&self, frame: &mut Frame<'_>, app: &App) -> Option<(u16, u16)> {
        let area = frame.area();
        let frame_block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(MAGENTA).add_modifier(Modifier::BOLD))
            .title(Line::from(vec![
                Span::styled("\u{25c8} ", Style::default().fg(CYAN)),
                Span::styled(parts::session(app), Style::default().fg(MAGENTA)),
                Span::styled(" \u{25c8} ", Style::default().fg(CYAN)),
            ]));
        let inner = frame_block.inner(area);
        frame.render_widget(frame_block, area);

        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(2),
                Constraint::Length(1),
            ])
            .split(inner);

        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format!("\u{2588}\u{2591} {} ", parts::rate(app)), Style::default().fg(CYAN)),
                Span::styled(format!("\u{2588}\u{2591} {} ", parts::tokens(app)), Style::default().fg(MAGENTA)),
                Span::styled(parts::model(app), Style::default().fg(Color::Rgb(0x9a, 0x9a, 0xff))),
            ])),
            rows[0],
        );

        let body: Vec<Line<'static>> = parts::transcript(app)
            .into_iter()
            .enumerate()
            .map(|(index, line)| {
                let text: String = line.spans.iter().map(|s| s.content.to_string()).collect();
                Line::from(vec![
                    Span::styled(
                        if index % 2 == 0 { "\u{2591}\u{2588} " } else { "\u{2588}\u{2591} " },
                        Style::default().fg(if index % 2 == 0 { MAGENTA } else { CYAN }),
                    ),
                    Span::styled(text, Style::default().fg(Color::Rgb(0xe8, 0xe8, 0xff))),
                ])
            })
            .collect();
        frame.render_widget(
            Paragraph::new(body)
                .wrap(Wrap { trim: false })
                .scroll((parts::scroll(app), 0)),
            rows[1],
        );

        let width = usize::from(rows[2].width.saturating_sub(3));
        let (scroll, column) = parts::input_view(app, width);
        frame.render_widget(
            Paragraph::new(format!("\u{203a}\u{203a} {}", parts::input(app)))
                .style(Style::default().fg(CYAN).add_modifier(Modifier::BOLD))
                .scroll((0, u16::try_from(scroll).unwrap_or(u16::MAX))),
            rows[2],
        );
        Some((rows[2].x + 3 + u16::try_from(column).unwrap_or(0), rows[2].y))
    }
}
