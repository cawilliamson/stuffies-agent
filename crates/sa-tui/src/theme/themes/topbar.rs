//! chrome across the top: title bar, prompt, then the transcript below.

use crate::theme::prelude::*;

pub struct Theme;

impl crate::theme::Theme for Theme {
    fn name(&self) -> &'static str {
        "topbar"
    }

    fn blurb(&self) -> &'static str {
        "everything at the top: title bar, prompt, then transcript"
    }

    fn render(&self, frame: &mut Frame<'_>, app: &App) -> Option<(u16, u16)> {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(3),
                Constraint::Min(3),
            ])
            .split(frame.area());

        let mut title = vec![
            Span::styled(" sa ", Style::default().fg(Color::Black).bg(Color::White)),
            Span::styled(
                format!(" {} ", parts::session(app)),
                Style::default().fg(Color::White).bg(Color::DarkGray),
            ),
        ];
        if let Some(branch) = parts::branch(app) {
            title.push(Span::styled(
                format!(" \u{e725} {branch} "),
                Style::default().fg(Color::White).bg(Color::Magenta),
            ));
        }
        title.push(Span::styled(
            format!(" {} ", parts::model(app)),
            Style::default().fg(Color::White).bg(Color::Blue),
        ));
        title.push(Span::styled(
            format!(" {} ", parts::rate(app)),
            Style::default().fg(Color::White).bg(Color::Red),
        ));
        title.push(Span::styled(
            format!(" {} ", parts::tokens(app)),
            Style::default().fg(Color::White).bg(Color::DarkGray),
        ));
        frame.render_widget(Paragraph::new(Line::from(title)), rows[0]);

        let width = usize::from(rows[1].width.saturating_sub(2));
        let (scroll, column) = parts::input_view(app, width);
        frame.render_widget(
            Paragraph::new(parts::input(app).to_owned())
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" prompt ")
                        .border_style(Style::default().fg(Color::White)),
                )
                .scroll((0, u16::try_from(scroll).unwrap_or(u16::MAX))),
            rows[1],
        );
        frame.render_widget(parts::transcript_paragraph(app), rows[2]);
        Some((rows[1].x + 1 + u16::try_from(column).unwrap_or(0), rows[1].y + 1))
    }
}
