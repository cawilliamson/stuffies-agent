//! a calm agent console: quiet transcript, boxed tool output, a clear composer and a
//! status line carrying the workspace.

use crate::theme::prelude::*;

pub struct Theme;

const RULE: Color = Color::Rgb(0x24, 0x28, 0x30);
const MUTED: Color = Color::Rgb(0x64, 0x6d, 0x7c);
const FAINT: Color = Color::Rgb(0x3a, 0x40, 0x4b);
const ACCENT: Color = Color::Rgb(0x8a, 0xc8, 0xff);

fn chip(text: &str, fg: Color, bg: Color) -> Span<'static> {
    Span::styled(format!(" {text} "), Style::default().fg(fg).bg(bg))
}

impl crate::theme::Theme for Theme {
    fn name(&self) -> &'static str {
        "studio"
    }

    fn blurb(&self) -> &'static str {
        "calm agent console: boxed tool output, composer and workspace status line"
    }

    fn transcript_rows(&self, area: Rect) -> u16 {
        area.height.saturating_sub(8)
    }

    fn render(&self, frame: &mut Frame<'_>, app: &App) -> Option<(u16, u16)> {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(3),
                Constraint::Length(3),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .split(frame.area());

        // header: wordmark, then the facts that matter, quietly.
        let mut header = vec![
            Span::styled(
                " sa ",
                Style::default().fg(Color::Rgb(0x10, 0x14, 0x1a)).bg(ACCENT),
            ),
            Span::raw("  "),
        ];
        let mut facts = vec![parts::session(app)];
        if let Some(branch) = parts::branch(app) {
            facts.push(branch);
        }
        if !parts::model(app).is_empty() {
            facts.push(parts::model(app));
        }
        for (index, fact) in facts.iter().enumerate() {
            if index > 0 {
                header.push(Span::styled("  \u{00b7}  ", Style::default().fg(FAINT)));
            }
            header.push(Span::styled(fact.clone(), Style::default().fg(MUTED)));
        }
        if parts::busy(app) {
            header.push(Span::styled(
                "   \u{25cf} working",
                Style::default().fg(Color::Rgb(0xe0, 0xb0, 0x60)),
            ));
        }
        frame.render_widget(Paragraph::new(Line::from(header)), rows[0]);
        frame.render_widget(
            Paragraph::new("\u{2500}".repeat(usize::from(rows[1].width)))
                .style(Style::default().fg(RULE)),
            rows[1],
        );

        frame.render_widget(
            Paragraph::new(parts::messages(app, &parts::Ink::default(), rows[2].width))
                .wrap(Wrap { trim: false })
                .scroll((parts::scroll(app), 0)),
            rows[2],
        );

        // composer.
        let width = usize::from(rows[3].width.saturating_sub(4));
        let (scroll, column) = parts::input_view(app, width);
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("\u{276f} ", Style::default().fg(ACCENT)),
                Span::raw(parts::input(app).to_owned()),
            ]))
            .scroll((0, u16::try_from(scroll).unwrap_or(u16::MAX)))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(RULE))
                    .padding(ratatui::widgets::Padding::new(1, 1, 0, 0)),
            ),
            rows[3],
        );

        // hint chips under the composer.
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                chip("enter", MUTED, Color::Rgb(0x1b, 0x1f, 0x26)),
                Span::raw(" send  "),
                chip("esc", MUTED, Color::Rgb(0x1b, 0x1f, 0x26)),
                Span::raw(" interrupt  "),
                chip("/", MUTED, Color::Rgb(0x1b, 0x1f, 0x26)),
                Span::raw(" commands  "),
                chip("ctrl-c", MUTED, Color::Rgb(0x1b, 0x1f, 0x26)),
                Span::raw(" quit"),
            ]))
            .style(Style::default().fg(MUTED)),
            rows[4],
        );

        // status line: where we are on the left, what is running on the right.
        let left = {
            let mut text = String::from(" sa");
            if let Some(workspace) = parts::workspace(app) {
                text.push_str(&format!("  {workspace}"));
            }
            if let Some(branch) = parts::branch(app) {
                text.push_str(&format!(" ({branch})"));
            }
            text
        };
        let right = format!(
            "{} in / {} out  \u{00b7}  {}  ",
            parts::compact(app.status.prompt_tokens),
            parts::compact(app.status.completion_tokens),
            parts::rate(app)
        );
        let used = left.chars().count() + right.chars().count();
        let pad = usize::from(rows[5].width).saturating_sub(used);
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    left,
                    Style::default().fg(Color::Rgb(0x14, 0x18, 0x1e)).bg(FAINT),
                ),
                Span::raw(" ".repeat(pad)),
                Span::styled(right, Style::default().fg(MUTED)),
            ])),
            rows[5],
        );

        Some((
            rows[3].x + 3 + u16::try_from(column).unwrap_or(0),
            rows[3].y + 1,
        ))
    }
}
