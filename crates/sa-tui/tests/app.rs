//! the tabbed tui: key handling, tab lifecycle, transcript framing and rate accounting.

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

fn ctrl_shift(c: char) -> KeyEvent {
    KeyEvent::new(
        KeyCode::Char(c),
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    )
}

fn typed(app: &mut App, text: &str) {
    for c in text.chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
}

fn screen(app: &mut App, width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("terminal");
    sa_tui::draw(&mut terminal, app).expect("draw");
    terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol())
        .collect()
}

#[test]
fn typing_and_enter_submits_the_prompt() {
    let mut app = App::new("session-abcdef", "m");
    typed(&mut app, "hello");
    assert_eq!(app.tab().input_text(), "hello");
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::Submit("hello".to_owned())
    );
    assert!(app.tab().input_text().is_empty());
}

#[test]
fn backspace_edits_the_input() {
    let mut app = App::new("s", "m");
    typed(&mut app, "abc");
    app.on_key(key(KeyCode::Backspace));
    assert_eq!(app.tab().input_text(), "ab");
}

#[test]
fn escape_interrupts_and_ctrl_c_quits() {
    let mut app = App::new("s", "m");
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::Cancel);
    assert_eq!(app.on_key(ctrl('c')), Action::Quit);
}

#[test]
fn shift_tab_requests_the_next_tab() {
    let mut app = App::new("s", "m");
    assert_eq!(
        app.on_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT)),
        Action::NextTab
    );
}

#[test]
fn shift_ctrl_n_and_w_request_tab_actions() {
    let mut app = App::new("s", "m");
    assert_eq!(app.on_key(ctrl_shift('N')), Action::NewTab);
    assert_eq!(app.on_key(ctrl_shift('W')), Action::CloseTab);
}

#[test]
fn tabs_can_be_added_cycled_and_closed() {
    let mut app = App::new("first", "m");
    app.add_tab("second", "m");
    app.add_tab("third", "m");
    assert_eq!(app.tabs.len(), 3);
    assert_eq!(app.active, 2);

    app.next_tab();
    assert_eq!(app.active, 0, "cycling wraps");
    app.next_tab();
    assert_eq!(app.active, 1);

    let closed = app.close_tab().expect("closes");
    assert_eq!(closed, "second");
    assert_eq!(app.tabs.len(), 2);
}

#[test]
fn the_last_tab_is_never_closed() {
    let mut app = App::new("only", "m");
    assert!(app.close_tab().is_none());
    assert_eq!(app.tabs.len(), 1);
}

#[test]
fn each_tab_keeps_its_own_transcript_and_input() {
    let mut app = App::new("first", "m");
    typed(&mut app, "one");
    app.add_tab("second", "m");
    typed(&mut app, "two");

    assert_eq!(app.tabs[0].input_text(), "one");
    assert_eq!(app.tabs[1].input_text(), "two");

    app.tab_mut().push(Role::User, "hello from second");
    assert_eq!(app.tabs[0].transcript.len(), 0);
    assert_eq!(app.tabs[1].transcript.len(), 1);
}

#[test]
fn events_route_to_the_tab_that_owns_the_session() {
    let mut app = App::new("first", "m");
    app.add_tab("second", "m");

    let tab = app.tab_for("first").expect("first tab");
    tab.on_event("llm.delta", &json!({"text": "hello"}));
    assert_eq!(app.tabs[0].transcript.len(), 1);
    assert_eq!(app.tabs[1].transcript.len(), 0);
}

#[test]
fn the_tab_bar_draws_tab_shapes() {
    let mut app = App::new("session-one", "m");
    app.add_tab("session-two", "m");

    let backend = TestBackend::new(100, 24);
    let mut terminal = Terminal::new(backend).expect("terminal");
    sa_tui::draw(&mut terminal, &mut app).expect("draw");
    let buffer = terminal.backend().buffer();
    let row = |y: u16| -> String { (0..100).map(|x| buffer[(x, y)].symbol()).collect() };

    let top = row(0);
    let mid = row(1);
    let bot = row(2);
    if std::env::var("SA_PREVIEW").is_ok() {
        println!("{top}\n{mid}\n{bot}");
    }

    assert!(
        top.contains('\u{256d}'),
        "the active tab needs a rounded top:\n{top}"
    );
    assert!(
        top.contains('\u{250c}'),
        "inactive tabs need square tops:\n{top}"
    );
    assert!(
        mid.contains('\u{2502}') && mid.contains('1'),
        "labels row:\n{mid}"
    );
    assert!(bot.contains('\u{2570}'), "tabs need bottoms:\n{bot}");
}

#[test]
fn prose_is_plain_and_only_tool_output_is_boxed() {
    let mut app = App::new("s", "m");
    app.tab_mut().push(Role::User, "a question");
    app.tab_mut().push(Role::Assistant, "an answer");
    app.tab_mut().push(Role::Tool, "bash ls");
    app.tab_mut().push(Role::ToolOutput, "file.txt");

    let screen = screen(&mut app, 100, 30);
    assert!(
        screen.contains("\u{276f} a question"),
        "user turn must be marked:\n{screen}"
    );
    assert!(screen.contains("an answer"), "{screen}");
    assert!(
        screen.contains("\u{25be} bash ls"),
        "tool call must be marked:\n{screen}"
    );
    assert!(screen.contains("file.txt"), "{screen}");

    // prose is prose: boxing every message made the transcript unreadable.
    assert!(
        !screen.contains("\u{256d}\u{2500} you"),
        "prose must not be boxed:\n{screen}"
    );
    assert!(
        !screen.contains("\u{256d}\u{2500} sa"),
        "prose must not be boxed:\n{screen}"
    );
    // but the tool output is in a well.
    assert!(
        screen.contains("\u{2502} file.txt"),
        "tool output must be framed:\n{screen}"
    );
}

#[test]
fn an_idle_tick_does_not_dirty_the_frame() {
    use std::time::{Duration, Instant};
    let mut app = App::new("s", "m");
    let before = app.revision();
    app.tab_mut()
        .tick_at(Instant::now() + Duration::from_secs(10));
    assert_eq!(
        app.revision(),
        before,
        "an idle tick must not force a redraw"
    );

    app.tab_mut().push(Role::Assistant, "new text");
    assert_ne!(app.revision(), before, "new content must dirty the frame");
}

#[test]
fn the_transcript_shows_no_raw_role_gutters() {
    let mut app = App::new("s", "m");
    app.tab_mut().push(Role::User, "q");
    app.tab_mut().push(Role::Assistant, "a");
    let screen = screen(&mut app, 100, 30);
    for gutter in [
        "you \u{2502}",
        "sa  \u{2502}",
        "tool\u{2502}",
        "sys \u{2502}",
    ] {
        assert!(!screen.contains(gutter), "raw gutter {gutter:?}:\n{screen}");
    }
}

#[test]
fn the_status_bar_shows_the_session_rate_and_tokens() {
    let mut app = App::new("session-abcdef", "deepseek");
    app.tab_mut().status.prompt_tokens = 230_000;
    app.tab_mut().status.completion_tokens = 1_500;
    let screen = screen(&mut app, 160, 24);
    assert!(screen.contains("session-"), "{screen}");
    assert!(screen.contains("0.0 t/s"), "{screen}");
    assert!(screen.contains("230k/1k"), "{screen}");
    assert!(
        screen.contains("shift+tab"),
        "key hints must be present:\n{screen}"
    );
}

#[test]
fn thinking_tokens_count_toward_the_rate() {
    use std::time::{Duration, Instant};
    let mut app = App::new("s", "m");
    let t0 = Instant::now();
    app.tab_mut()
        .on_event_at("turn.started", &json!({"prompt": "go"}), t0);
    app.tab_mut()
        .on_event_at("llm.reasoning", &json!({"text": "y".repeat(400)}), t0);
    app.tab_mut()
        .on_event_at("llm.completed", &json!({}), t0 + Duration::from_secs(2));
    app.tab_mut().on_event_at(
        "turn.completed",
        &json!({"usage": {"prompt_tokens": 0, "completion_tokens": 100}}),
        t0 + Duration::from_secs(2),
    );
    app.tab_mut().tick_at(t0 + Duration::from_secs(2));
    assert!(
        (app.tab().tok_per_sec - 50.0).abs() < 1.0,
        "got {}",
        app.tab().tok_per_sec
    );
}

#[test]
fn the_average_counts_only_generation_time() {
    use std::time::{Duration, Instant};
    let mut app = App::new("s", "m");
    let t0 = Instant::now();
    let done = t0 + Duration::from_secs(2);
    let after = done + Duration::from_secs(28);
    app.tab_mut()
        .on_event_at("turn.started", &json!({"prompt": "go"}), t0);
    app.tab_mut()
        .on_event_at("llm.delta", &json!({"text": "x".repeat(400)}), t0);
    app.tab_mut().on_event_at("llm.completed", &json!({}), done);
    app.tab_mut()
        .on_event_at("tool.requested", &json!({"tool": "bash"}), done);
    app.tab_mut()
        .on_event_at("tool.completed", &json!({"output": "slow"}), after);
    app.tab_mut().on_event_at(
        "turn.completed",
        &json!({"usage": {"prompt_tokens": 1, "completion_tokens": 100}}),
        after,
    );
    app.tab_mut().tick_at(after);
    assert!(
        (app.tab().tok_per_sec - 50.0).abs() < 1.0,
        "tool time must not count: got {}",
        app.tab().tok_per_sec
    );
}

#[test]
fn the_average_holds_until_the_next_message_starts() {
    use std::time::{Duration, Instant};
    let mut app = App::new("s", "m");
    let t0 = Instant::now();
    let done = t0 + Duration::from_secs(2);
    app.tab_mut()
        .on_event_at("turn.started", &json!({"prompt": "go"}), t0);
    app.tab_mut()
        .on_event_at("llm.delta", &json!({"text": "x".repeat(400)}), t0);
    app.tab_mut().on_event_at("llm.completed", &json!({}), done);
    app.tab_mut().on_event_at(
        "turn.completed",
        &json!({"usage": {"prompt_tokens": 1, "completion_tokens": 100}}),
        done,
    );

    app.tab_mut().tick_at(done + Duration::from_secs(30));
    assert!(app.tab().tok_per_sec > 0.0, "the figure holds");

    app.tab_mut().on_event_at(
        "turn.started",
        &json!({"prompt": "next"}),
        done + Duration::from_secs(31),
    );
    app.tab_mut().tick_at(done + Duration::from_secs(31));
    assert_eq!(app.tab().tok_per_sec, 0.0, "a new message resets it");
}

#[test]
fn scroll_clamps_to_the_rendered_transcript() {
    let mut app = App::new("s", "m");
    for n in 0..12 {
        app.tab_mut().push(Role::Assistant, format!("line {n}"));
    }
    let _ = screen(&mut app, 100, 24);
    let ceiling = app.tab().max_scroll;
    assert!(ceiling > 0, "a long transcript must be scrollable");

    for _ in 0..500 {
        app.on_key(key(KeyCode::Up));
    }
    assert_eq!(app.tab().scroll, 0);

    for _ in 0..500 {
        app.on_key(key(KeyCode::Down));
    }
    assert_eq!(app.tab().scroll, ceiling);
}

#[test]
fn the_layout_renders_at_several_sizes() {
    for (width, height) in [(20u16, 6u16), (40, 10), (100, 30), (200, 60)] {
        let mut app = App::new("session-abcdef", "deepseek");
        app.branch = Some("main".to_owned());
        app.add_tab("second-session", "deepseek");
        app.tab_mut().push(Role::User, "hello there");
        app.tab_mut().push(
            Role::Assistant,
            "a reply long enough to wrap in a narrow pane",
        );
        app.tab_mut().push(Role::ToolOutput, "output line");
        let _ = screen(&mut app, width, height);
    }
}
