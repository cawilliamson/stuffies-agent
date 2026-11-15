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
fn the_tab_bar_is_one_row_of_lit_and_dim_tabs() {
    let mut app = App::new("session-one", "m");
    app.add_tab("session-two", "m");

    let backend = TestBackend::new(100, 24);
    let mut terminal = Terminal::new(backend).expect("terminal");
    sa_tui::draw(&mut terminal, &mut app).expect("draw");
    let buffer = terminal.backend().buffer();

    let row0: String = (0..100).map(|x| buffer[(x, 0)].symbol()).collect();
    assert!(row0.contains("1 session"), "first tab label:\n{row0}");
    assert!(row0.contains("2 session"), "second tab label:\n{row0}");
    assert!(row0.contains('\u{2502}'), "tabs need a divider:\n{row0}");

    // the active tab is a lit chip; the inactive one is not filled.
    let lit: Vec<&str> = buffer
        .content()
        .iter()
        .filter(|c| c.bg == sa_tui::app::ACCENT)
        .map(|c| c.symbol())
        .collect();
    assert!(!lit.is_empty(), "the active tab must be highlighted");

    // one row only: the pane border starts on row 1.
    let row1: String = (0..100).map(|x| buffer[(x, 1)].symbol()).collect();
    assert!(
        row1.contains('\u{256d}'),
        "the pane must start immediately below:\n{row1}"
    );
}

#[test]
fn every_transcript_entry_is_framed_with_its_role() {
    let mut app = App::new("s", "deepseek-v4-flash");
    for (role, text) in [
        (Role::User, "a question"),
        (Role::Assistant, "an answer"),
        (Role::Thinking, "reasoning here"),
        (Role::Tool, "bash ls"),
        (Role::ToolOutput, "file.txt"),
        (Role::Error, "it broke"),
    ] {
        app.tab_mut().push(role, text);
    }

    let screen = screen(&mut app, 100, 40);
    for label in [
        "\u{256d}\u{2500} you",
        "\u{256d}\u{2500} deepseek-v4-flash",
        "\u{256d}\u{2500} thinking",
        "\u{256d}\u{2500} tool",
        "\u{256d}\u{2500} output",
        "\u{256d}\u{2500} error",
    ] {
        assert!(screen.contains(label), "missing frame {label:?}:\n{screen}");
    }
    for text in [
        "a question",
        "an answer",
        "reasoning here",
        "file.txt",
        "it broke",
    ] {
        assert!(screen.contains(text), "missing body {text:?}");
    }
}

#[test]
fn each_role_gets_a_distinct_frame_colour() {
    use ratatui::style::Color;
    let mut app = App::new("s", "m");
    for (role, text) in [
        (Role::User, "q"),
        (Role::Assistant, "a"),
        (Role::Thinking, "t"),
        (Role::Tool, "c"),
        (Role::Error, "e"),
    ] {
        app.tab_mut().push(role, text);
    }

    let backend = TestBackend::new(100, 40);
    let mut terminal = Terminal::new(backend).expect("terminal");
    sa_tui::draw(&mut terminal, &mut app).expect("draw");
    let buffer = terminal.backend().buffer();

    let mut edges: Vec<Color> = buffer
        .content()
        .iter()
        .filter(|c| c.symbol() == "\u{256d}")
        .map(|c| c.fg)
        .collect();
    edges.sort_by_key(|c| format!("{c:?}"));
    edges.dedup();
    assert!(
        edges.len() >= 5,
        "expected a distinct frame colour per role, got {edges:?}"
    );
}

#[test]
fn thinking_text_is_rendered_and_boxed() {
    let mut app = App::new("s", "m");
    app.tab_mut()
        .on_event("llm.reasoning", &json!({"text": "considering options"}));
    let screen = screen(&mut app, 100, 30);
    assert!(screen.contains("thinking"), "{screen}");
    assert!(screen.contains("considering options"), "{screen}");
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

#[test]
fn the_composer_is_just_a_box_with_the_text() {
    let mut app = App::new("session-abcdef", "m");
    typed(&mut app, "hello");

    let height = 24u16;
    let backend = TestBackend::new(100, height);
    let mut terminal = Terminal::new(backend).expect("terminal");
    sa_tui::draw(&mut terminal, &mut app).expect("draw");
    let buffer = terminal.backend().buffer();
    let row = |y: u16| -> String { (0..100).map(|x| buffer[(x, y)].symbol()).collect() };

    let composer = row(height - 3);
    assert!(
        composer.contains("hello"),
        "the text must render:\n{composer}"
    );
    // the caret is the terminal cursor, not a drawn glyph.
    assert!(!composer.contains('|'), "no drawn caret:\n{composer}");
    assert!(
        !composer.contains('\u{276f}'),
        "no prompt glyph:\n{composer}"
    );

    let border = row(height - 4);
    assert!(
        border.contains('\u{256d}') && border.contains('\u{256e}'),
        "the box must remain:\n{border}"
    );
}

#[test]
fn every_frame_row_is_the_same_width() {
    let mut app = App::new("s", "m");
    app.tab_mut().push(Role::User, "a question");
    app.tab_mut().push(Role::Assistant, "an answer");
    app.tab_mut().push(Role::Thinking, "thinking");

    let lines = sa_tui::render(&app, 98);
    let widths: Vec<usize> = lines.iter().map(|l| l.width()).collect();
    let first = widths.first().copied().unwrap_or(0);
    assert_eq!(first, 98, "frames must fill the pane exactly");
    for (index, width) in widths.iter().enumerate() {
        assert_eq!(
            *width, first,
            "row {index} is {width} wide, expected {first}"
        );
    }
}

#[test]
fn the_assistant_frame_is_labelled_with_the_model_not_the_agent() {
    let mut app = App::new("s", "deepseek-v4-flash");
    app.tab_mut().push(Role::Assistant, "an answer");
    let screen = screen(&mut app, 100, 20);

    assert!(
        screen.contains("\u{256d}\u{2500} deepseek-v4-flash"),
        "the model must label its own output:\n{screen}"
    );
    assert!(
        !screen.contains("\u{256d}\u{2500} sa "),
        "the agent's own name must not appear:\n{screen}"
    );
}

#[test]
fn an_unknown_model_falls_back_to_a_generic_label() {
    let mut app = App::new("s", "");
    app.tab_mut().push(Role::Assistant, "an answer");
    let screen = screen(&mut app, 100, 20);
    assert!(screen.contains("\u{256d}\u{2500} assistant"), "{screen}");
}

#[test]
fn reasoning_and_its_reply_share_one_split_frame() {
    let mut app = App::new("s", "deepseek-v4-flash");
    app.tab_mut().push(Role::Thinking, "weighing the options");
    app.tab_mut().push(Role::Assistant, "here is the answer");

    let screen = screen(&mut app, 100, 30);
    // one titled frame, not two.
    // the pane border also starts with a corner, so match the frame header's space.
    assert_eq!(
        screen.matches("\u{256d}\u{2500} ").count(),
        1,
        "the turn must be a single frame:\n{screen}"
    );
    assert!(
        screen.contains("\u{256d}\u{2500} deepseek-v4-flash"),
        "{screen}"
    );
    assert!(screen.contains("weighing the options"), "{screen}");
    assert!(screen.contains("here is the answer"), "{screen}");
    assert!(
        screen.contains('\u{251c}'),
        "the two sections must be split by a rule:\n{screen}"
    );

    // reasoning sits above the reply.
    let thinking_at = screen.find("weighing the options").expect("reasoning");
    let reply_at = screen.find("here is the answer").expect("reply");
    assert!(
        thinking_at < reply_at,
        "reasoning must come first:\n{screen}"
    );
}

#[test]
fn reasoning_without_a_reply_keeps_its_own_frame() {
    let mut app = App::new("s", "deepseek-v4-flash");
    app.tab_mut().push(Role::Thinking, "still thinking");
    let screen = screen(&mut app, 100, 20);
    assert!(screen.contains("\u{256d}\u{2500} thinking"), "{screen}");
    assert!(screen.contains("still thinking"), "{screen}");
    assert!(
        !screen.contains('\u{251c}'),
        "no divider without a reply:\n{screen}"
    );
}
