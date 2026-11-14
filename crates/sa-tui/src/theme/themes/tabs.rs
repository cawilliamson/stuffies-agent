//! browser-style tab strip across the top with a content well beneath.

use crate::theme::prelude::*;

pub struct Theme;

impl crate::theme::Theme for Theme {
    fn name(&self) -> &'static str {
        "tabs"
    }

    fn blurb(&self) -> &'static str {
        "tab strip header over a content well, address-bar prompt"
    }

    fn render(&self, frame: &mut Frame<'_>, app: &App) -> Option<(u16, u16)> {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(3),
                Constraint::Min(3),
                Constraint::Length(1),
            ])
            .split(frame.area());

        let mut tabs = vec![Span::styled(
            format!(" {} \u{2715} ", parts::session(app)),
            Style::default().fg(Color::Black).bg(Color::White).add_modifier(Modifier::BOLD),
        )];
        if let Some(branch) = parts::branch(app) {
            tabs.push(Span::styled(
                format!(" {branch} "),
                Style::default().fg(Color::Gray).bg(Color::Rgb(0x30, 0x30, 0x30)),
            ));
        }
        tabs.push(Span::styled(" + ", Style::default().fg(Color::DarkGray)));
        frame.render_widget(Paragraph::new(Line::from(tabs)), rows[0]);

        let width = usize::from(rows[1].width.saturating_sub(4));
        let (scroll, column) = parts::input_view(app, width);
        frame.render_widget(
            Paragraph::new(format!(" \u{1f310} {}", parts::input(app)))
                .scroll((0, u16::try_from(scroll).unwrap_or(u16::MAX)))
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_type(BorderType::Rounded)
                        .border_style(Style::default().fg(Color::DarkGray)),
                ),
            rows[1],
        );

        frame.render_widget(
            Paragraph::new(parts::transcript(app))
                .wrap(Wrap { trim: false })
                .scroll((parts::scroll(app), 0))
                .block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(Color::Rgb(0x30, 0x30, 0x30)))),
            rows[2],
        );

        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format!(" {} ", parts::model(app)), Style::default().fg(Color::DarkGray)),
                Span::styled(format!(" {} ", parts::rate(app)), Style::default().fg(Color::Red)),
                Span::styled(format!(" {} ", parts::tokens(app)), Style::default().fg(Color::DarkGray)),
            ])),
            rows[3],
        );
        Some((rows[1].x + 3 + u16::try_from(column).unwrap_or(0), rows[1].y + 1))
    }
}
