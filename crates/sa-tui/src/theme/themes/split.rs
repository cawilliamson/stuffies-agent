//! vertical split: transcript on the left, prompt and facts on the right.

use crate::theme::prelude::*;

pub struct Theme;

impl crate::theme::Theme for Theme {
    fn name(&self) -> &'static str {
        "split"
    }

    fn blurb(&self) -> &'static str {
        "side-by-side split: transcript left, prompt and facts right"
    }

    fn render(&self, frame: &mut Frame<'_>, app: &App) -> Option<(u16, u16)> {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
            .split(frame.area());
        let right = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(5),
                Constraint::Length(5),
                Constraint::Min(3),
            ])
            .split(cols[1]);

        frame.render_widget(
            Paragraph::new(parts::transcript(app))
                .wrap(Wrap { trim: false })
                .scroll((parts::scroll(app), 0))
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" transcript ")
                        .border_style(Style::default().fg(Color::DarkGray)),
                ),
            cols[0],
        );

        frame.render_widget(
            Paragraph::new(vec![
                Line::from(format!("{}  {}", parts::session(app), parts::rate(app))),
                Line::from(parts::model(app)),
                Line::from(parts::tokens(app)),
            ])
            .block(Block::default().borders(Borders::ALL).title(" facts ")),
            right[0],
        );

        let width = usize::from(right[1].width.saturating_sub(2));
        let (scroll, column) = parts::input_view(app, width);
        frame.render_widget(
            Paragraph::new(parts::input(app).to_owned())
                .block(Block::default().borders(Borders::ALL).title(" prompt "))
                .scroll((0, u16::try_from(scroll).unwrap_or(u16::MAX))),
            right[1],
        );
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled("esc interrupt", Style::default().fg(Color::DarkGray))),
                Line::from(Span::styled("ctrl-c quit", Style::default().fg(Color::DarkGray))),
                Line::from(Span::styled("/theme to switch", Style::default().fg(Color::DarkGray))),
            ])
            .block(Block::default().borders(Borders::ALL).title(" keys ")),
            right[2],
        );

        Some((right[1].x + 1 + u16::try_from(column).unwrap_or(0), right[1].y + 1))
    }
}
