//! a persistent left rail with session facts, transcript to its right.

use crate::theme::prelude::*;

pub struct Theme;

fn rail(app: &App) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(Span::styled(" SESSION ", Style::default().fg(Color::Black).bg(Color::Cyan))),
        Line::from(format!(" {}", parts::session(app))),
        Line::from(""),
    ];
    if let Some(branch) = parts::branch(app) {
        lines.push(Line::from(Span::styled(" BRANCH ", Style::default().fg(Color::Black).bg(Color::Magenta))));
        lines.push(Line::from(format!(" {branch}")));
        lines.push(Line::from(""));
    }
    lines.push(Line::from(Span::styled(" MODEL ", Style::default().fg(Color::Black).bg(Color::Blue))));
    lines.push(Line::from(format!(" {}", parts::model(app))));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(" RATE ", Style::default().fg(Color::Black).bg(Color::Red))));
    lines.push(Line::from(format!(" {}", parts::rate(app))));
    lines.push(Line::from(format!(" {}", parts::tokens(app))));
    lines
}

impl crate::theme::Theme for Theme {
    fn name(&self) -> &'static str {
        "sidebar-left"
    }

    fn blurb(&self) -> &'static str {
        "left rail of session facts, transcript and prompt on the right"
    }

    fn render(&self, frame: &mut Frame<'_>, app: &App) -> Option<(u16, u16)> {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(22), Constraint::Min(20)])
            .split(frame.area());
        let right = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(3), Constraint::Length(3)])
            .split(cols[1]);

        frame.render_widget(
            Paragraph::new(rail(app)).block(
                Block::default()
                    .borders(Borders::RIGHT)
                    .border_style(Style::default().fg(Color::DarkGray)),
            ),
            cols[0],
        );
        frame.render_widget(parts::transcript_paragraph(app), right[0]);

        let width = usize::from(right[1].width.saturating_sub(2));
        let (scroll, column) = parts::input_view(app, width);
        frame.render_widget(
            Paragraph::new(parts::input(app).to_owned())
                .block(Block::default().borders(Borders::ALL).title(" prompt "))
                .scroll((0, u16::try_from(scroll).unwrap_or(u16::MAX))),
            right[1],
        );
        Some((right[1].x + 1 + u16::try_from(column).unwrap_or(0), right[1].y + 1))
    }
}
