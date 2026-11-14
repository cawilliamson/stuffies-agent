//! the tui state machine and its rendering (spec 14, v1.1).

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use sa_tui::app::{Action, App, Role};
use serde_json::json;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

fn typed(app: &mut App, text: &str) {
    for c in text.chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
}

#[test]
fn typing_and_enter_submits_the_prompt() {
    let mut app = App::new("s", "m");
    typed(&mut app, "hello");
    assert_eq!(app.input, "hello");
    let action = app.on_key(key(KeyCode::Enter));
    assert_eq!(action, Action::Submit("hello".to_owned()));
    assert!(app.input.is_empty(), "input must be cleared on submit");
}

#[test]
fn enter_on_an_empty_input_does_nothing() {
    let mut app = App::new("s", "m");
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
}

#[test]
fn backspace_edits_the_input() {
    let mut app = App::new("s", "m");
    typed(&mut app, "abc");
    app.on_key(key(KeyCode::Backspace));
    assert_eq!(app.input, "ab");
}

#[test]
fn ctrl_c_cancels_a_running_turn() {
    let mut app = App::new("s", "m");
    assert_eq!(app.on_key(ctrl('c')), Action::Cancel);
}

#[test]
fn ctrl_c_denies_a_pending_approval_instead_of_cancelling() {
    let mut app = App::new("s", "m");
    app.pending_approval = Some("a1".to_owned());
    assert_eq!(
        app.on_key(ctrl('c')),
        Action::Answer {
            allow: false,
            scope: "once"
        }
    );
    assert!(app.pending_approval.is_none());
}

#[test]
fn approval_scopes_are_selectable() {
    for (typed_answer, allow, scope) in [
        ("y", true, "once"),
        ("t", true, "turn"),
        ("s", true, "session"),
        ("c", true, "class"),
        ("n", false, "once"),
    ] {
        let mut app = App::new("s", "m");
        app.pending_approval = Some("a1".to_owned());
        typed(&mut app, typed_answer);
        assert_eq!(
            app.on_key(key(KeyCode::Enter)),
            Action::Answer { allow, scope },
            "answer {typed_answer:?}"
        );
    }
}

#[test]
fn ctrl_d_quits() {
    let mut app = App::new("s", "m");
    assert_eq!(app.on_key(ctrl('d')), Action::Quit);
}

#[test]
fn scroll_clamps_to_the_transcript() {
    let mut app = App::new("s", "m");
    for n in 0..3 {
        app.push(Role::System, format!("line {n}"));
    }
    for _ in 0..50 {
        app.on_key(key(KeyCode::Up));
    }
    assert_eq!(app.scroll, 0, "scrolling up must stop at the top");
    for _ in 0..50 {
        app.on_key(key(KeyCode::Down));
    }
    assert_eq!(app.scroll, 2, "scrolling down must stop at the last entry");
}

#[test]
fn tool_events_render_as_transcript_entries() {
    let mut app = App::new("s", "m");
    app.on_event(
        "tool.requested",
        &json!({"tool": "read", "args": {"path": "main.rs"}}),
    );
    app.on_event("tool.completed", &json!({"output": "fn main() {}"}));

    let text: String = app
        .lines()
        .iter()
        .map(|l| {
            l.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("→ read main.rs"), "{text}");
    assert!(text.contains("fn main() {}"), "{text}");
}

#[test]
fn streamed_text_accumulates_into_one_assistant_entry() {
    let mut app = App::new("s", "m");
    app.on_event("llm.delta", &json!({"text": "hel"}));
    app.on_event("llm.delta", &json!({"text": "lo"}));
    let assistant: Vec<&str> = app
        .transcript
        .iter()
        .filter(|e| e.role == Role::Assistant)
        .map(|e| e.text.as_str())
        .collect();
    assert_eq!(assistant, vec!["hello"]);
}

#[test]
fn usage_accumulates_across_turns() {
    let mut app = App::new("s", "m");
    app.status.turn_running = true;
    app.on_event(
        "turn.completed",
        &json!({"usage": {"prompt_tokens": 10, "completion_tokens": 5}}),
    );
    assert_eq!(app.status.prompt_tokens, 10);
    assert!(!app.status.turn_running);
}

#[test]
fn the_input_line_renders() {
    let mut app = App::new("session-abcdef", "deepseek");
    app.push(Role::User, "what is this repo");
    typed(&mut app, "next prompt");

    let backend = TestBackend::new(80, 20);
    let mut terminal = Terminal::new(backend).expect("terminal");
    sa_tui::draw(&mut terminal, &app).expect("draw");

    let buffer = terminal.backend().buffer();
    let screen: String = buffer.content().iter().map(|cell| cell.symbol()).collect();
    assert!(screen.contains("what is this repo"), "{screen}");
    assert!(screen.contains("next prompt"), "{screen}");
    assert!(
        screen.contains("deepseek"),
        "status bar must show the model"
    );
}

fn user_entries(app: &App) -> usize {
    app.transcript
        .iter()
        .filter(|e| e.role == Role::User)
        .count()
}

#[test]
fn submitting_does_not_render_the_prompt_twice() {
    // the prompt is echoed locally when submitted, and the daemon also emits turn.started.
    // only the local echo may render, or the operator sees their message duplicated.
    let mut app = App::new("s", "m");
    app.push(Role::User, "hello");
    app.on_event("turn.started", &json!({"prompt": "hello"}));
    assert_eq!(user_entries(&app), 1, "{:?}", app.transcript);
}

#[test]
fn replay_still_reconstructs_prompts_from_the_log() {
    let mut app = App::new("s", "m");
    app.replaying = true;
    app.on_event("turn.started", &json!({"prompt": "first"}));
    app.on_event("turn.started", &json!({"prompt": "second"}));
    app.replaying = false;
    assert_eq!(user_entries(&app), 2);
}

#[test]
fn every_role_renders_a_distinct_gutter() {
    let mut app = App::new("s", "m");
    app.push(Role::User, "mine");
    app.push(Role::Assistant, "theirs");
    app.push(Role::Tool, "tool output");

    let screen: String = app
        .lines()
        .iter()
        .map(|l| {
            l.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");

    assert!(screen.contains("you │ mine"), "{screen}");
    assert!(screen.contains("sa  │ theirs"), "{screen}");
    assert!(screen.contains("tool│ tool output"), "{screen}");
}

#[test]
fn the_agent_and_the_operator_use_different_colours() {
    use ratatui::style::Color;
    let mut app = App::new("s", "m");
    app.push(Role::User, "mine");
    app.push(Role::Assistant, "theirs");
    let colours: Vec<Color> = app
        .lines()
        .iter()
        .filter_map(|line| line.spans.first().and_then(|s| s.style.fg))
        .collect();
    assert!(colours.contains(&Color::Cyan), "{colours:?}");
    assert!(colours.contains(&Color::Green), "{colours:?}");
}
