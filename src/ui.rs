use ratatui::layout::{Alignment, Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState};
use ratatui::Frame;

use crate::macro_def::{vk_name, KeyAction, Macro, SendMode, StopCondition};
use crate::state::{AppState, ConfigField, View};
use crate::MAIN_BUTTONS;

const ACCENT: Color = Color::Rgb(200, 200, 210);
const GREEN: Color = Color::Rgb(80, 220, 120);
const RED: Color = Color::Rgb(255, 90, 90);
const YELLOW: Color = Color::Rgb(255, 210, 80);
const GRAY: Color = Color::DarkGray;
const DIM: Color = Color::Rgb(100, 100, 100);
const WHITE: Color = Color::White;
const CYAN: Color = Color::Rgb(80, 220, 220);
const ORANGE: Color = Color::Rgb(255, 160, 50);
const BG_SELECTED: Color = Color::Rgb(40, 50, 70);
const FG_TEXT: Color = Color::Rgb(200, 200, 200);
const FG_TEXT_DIM: Color = Color::Rgb(180, 180, 180);
const BG_ACTIVE_ROW: Color = Color::Rgb(180, 120, 40);

/// Render the current view for one frame.
pub fn draw(f: &mut Frame, state: &mut AppState) {
    match &state.view {
        View::Main => draw_main(f, state),
        View::Recording => draw_recording(f, state),
        View::Config(idx) => draw_config(f, state, *idx),
        View::StepEditor(idx) => draw_step_editor(f, state, *idx),
        View::ProcessPicker(_) => draw_process_picker(f, state),
        View::ChainPicker(_) => draw_chain_picker(f, state),
        View::Help => draw_help(f, state),
    }
}

fn status_bar(state: &AppState) -> Line<'_> {
    let macros_span = if state.macros_enabled {
        Span::styled(" MACROS ON ", Style::default().fg(Color::Black).bg(GREEN))
    } else {
        Span::styled(" MACROS OFF ", Style::default().fg(WHITE).bg(RED))
    };

    let audio_span = if state.audio_enabled {
        Span::styled(" AUDIO ", Style::default().fg(Color::Black).bg(ACCENT))
    } else {
        Span::styled(" MUTE ", Style::default().fg(WHITE).bg(DIM))
    };

    let speed_span = if let Some(display) = state.speed_display() {
        Span::styled(
            format!(" x{:.2} ", display),
            Style::default().fg(Color::Black).bg(YELLOW),
        )
    } else {
        Span::styled("", Style::default())
    };

    Line::from(vec![
        macros_span,
        Span::raw(" "),
        audio_span,
        Span::raw(" "),
        speed_span,
    ])
}

fn system_stats_title(state: &AppState) -> Line<'_> {
    let cpu_color = if state.cpu_usage > 80 { RED } else if state.cpu_usage > 50 { YELLOW } else { DIM };
    let ram_pct = (state.ram_used_mb * 100).checked_div(state.ram_total_mb).unwrap_or(0);
    let ram_color = if ram_pct > 85 { RED } else if ram_pct > 60 { YELLOW } else { DIM };

    let secs = state.uptime_secs;
    let uptime = if secs >= 3600 {
        format!("{}h{:02}m", secs / 3600, (secs % 3600) / 60)
    } else if secs >= 60 {
        format!("{}m{:02}s", secs / 60, secs % 60)
    } else {
        format!("{}s", secs)
    };

    Line::from(vec![
        Span::styled(format!(" CPU {}%", state.cpu_usage), Style::default().fg(cpu_color)),
        Span::styled(" | ", Style::default().fg(DIM)),
        Span::styled(
            format!("RAM {:.1}/{:.1}GB", state.ram_used_mb as f32 / 1024.0, state.ram_total_mb as f32 / 1024.0),
            Style::default().fg(ram_color),
        ),
        Span::styled(" | ", Style::default().fg(DIM)),
        Span::styled(format!("UP {} ", uptime), Style::default().fg(DIM)),
    ])
}

/// Build a title_bottom Line from the status message, or empty if none.
fn status_bottom_line(state: &AppState) -> Line<'_> {
    if let Some((msg, _, _)) = &state.status_message {
        Line::from(vec![
            Span::styled(" ", Style::default()),
            Span::styled(msg.as_str(), Style::default().fg(YELLOW).add_modifier(Modifier::BOLD)),
            Span::styled(" ", Style::default()),
        ])
    } else {
        Line::from("")
    }
}

fn button_bar_line<'a>(buttons: &'a [&'a str], selected: usize) -> Line<'a> {
    let mut spans = Vec::new();
    for (i, label) in buttons.iter().enumerate() {
        if i == selected {
            spans.push(Span::styled(
                format!(" {} ", label),
                Style::default()
                    .fg(Color::Black)
                    .bg(ACCENT)
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            spans.push(Span::styled(
                format!(" {} ", label),
                Style::default().fg(FG_TEXT_DIM),
            ));
        }
        if i < buttons.len() - 1 {
            spans.push(Span::styled("\u{2502}", Style::default().fg(DIM)));
        }
    }
    Line::from(spans)
}

/// Split the frame into a one-line status bar (rendered here) plus `body`
/// constraints; the returned chunks index the body from 1.
fn split_with_status_bar(
    f: &mut Frame,
    state: &AppState,
    body: &[Constraint],
) -> std::rc::Rc<[ratatui::layout::Rect]> {
    let mut constraints = vec![Constraint::Length(1)];
    constraints.extend_from_slice(body);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(f.area());
    f.render_widget(Paragraph::new(status_bar(state)), chunks[0]);
    chunks
}

/// Render a bordered one-line footer of spans into `area`.
fn render_footer(f: &mut Frame, area: ratatui::layout::Rect, border: Style, spans: Vec<Span>) {
    let block = Block::default().borders(Borders::ALL).border_style(border);
    let inner = block.inner(area);
    f.render_widget(block, area);
    f.render_widget(Paragraph::new(Line::from(spans)), inner);
}

/// Format "N/M" position or a fallback for empty lists.
fn format_pos(selected: usize, total: usize, empty: &str) -> String {
    if total > 0 {
        format!(" {}/{} ", selected + 1, total)
    } else {
        format!(" {} ", empty)
    }
}

/// Choose status message or fallback position text for title_bottom.
fn status_or_pos<'a>(state: &'a AppState, pos: String) -> Line<'a> {
    if state.status_message.is_some() {
        status_bottom_line(state)
    } else {
        Line::from(Span::styled(pos, Style::default().fg(DIM)))
    }
}

fn row_style(selected: bool) -> Style {
    if selected {
        Style::default().fg(WHITE).bg(BG_SELECTED).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(FG_TEXT)
    }
}

fn help_key<'a>(key: &'a str, label: &'a str, color: Color) -> Vec<Span<'a>> {
    vec![
        Span::styled(key, Style::default().fg(color).add_modifier(Modifier::BOLD)),
        Span::styled(format!(" {}  ", label), Style::default().fg(GRAY)),
    ]
}

const BORDER_STYLE: Style = Style::new().fg(DIM);

fn draw_main(f: &mut Frame, state: &AppState) {
    let area = f.area();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // status bar block
            Constraint::Min(5),    // macro table
            Constraint::Length(3), // toolbar block
        ])
        .split(area);

    let status_block = Block::default()
        .borders(Borders::ALL)
        .border_style(BORDER_STYLE)
        .title(Span::styled(" tapmatic ", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)))
        .title(system_stats_title(state).alignment(Alignment::Right));
    let status_inner = status_block.inner(chunks[0]);
    f.render_widget(status_block, chunks[0]);
    f.render_widget(Paragraph::new(status_bar(state)), status_inner);

    let status_line = status_bottom_line(state);

    let filtered = state.filtered_macro_indices();

    if filtered.is_empty() {
        let msg = if !state.search_query.is_empty() {
            format!("No macros matching \"{}\"", state.search_query)
        } else {
            "No macros yet".into()
        };
        let hint = if state.search_query.is_empty() {
            "Press Enter on [Record] or Space to start"
        } else {
            "Press Esc to clear search"
        };
        let content = vec![
            Line::from(""),
            Line::from(vec![Span::styled(
                msg,
                Style::default().fg(DIM).add_modifier(Modifier::BOLD),
            )]),
            Line::from(""),
            Line::from(vec![Span::styled(
                hint,
                Style::default().fg(GRAY),
            )]),
        ];
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(BORDER_STYLE)
            .title(Span::styled(" Macros ", Style::default().fg(DIM)))
            .title_bottom(status_line);
        f.render_widget(
            Paragraph::new(content)
                .alignment(Alignment::Center)
                .block(block),
            chunks[1],
        );
    } else {
        let header = Row::new(vec![
            Cell::from("  # "),
            Cell::from("Name"),
            Cell::from("Key"),
            Cell::from("Repeat"),
            Cell::from("Delay"),
            Cell::from("Stop"),
            Cell::from("Window"),
            Cell::from("Status"),
        ])
        .style(Style::default().fg(ACCENT).add_modifier(Modifier::BOLD));

        let rows: Vec<Row> = filtered
            .iter()
            .enumerate()
            .map(|(display_i, &real_i)| {
                let mac = &state.macros[real_i];
                let selected = display_i == state.selected_macro;
                let active = state.is_macro_active(mac.hotkey_vk);
                let cursor = if selected { "> " } else { "  " };
                let num = format!("{}", real_i + 1);

                let hotkey = if mac.hotkey_vk != 0 {
                    vk_name(mac.hotkey_vk).to_string()
                } else {
                    "---".into()
                };
                let mode = mac.repetition.label();
                let delay: String = if let Some((min, max)) = mac.random_delay {
                    format!("~{}-{}ms", min, max)
                } else if mac.use_recorded_delays {
                    "Rec".into()
                } else {
                    format!("{}ms", mac.fixed_interval_ms)
                };
                let stop: String = match mac.stop_condition {
                    StopCondition::None => "---".into(),
                    StopCondition::AfterReps(n) => format!("{}x", n),
                    StopCondition::AfterSecs(n) => format!("{}s", n),
                };
                let process: String = match (&mac.bound_process, mac.send_mode) {
                    (Some(p), SendMode::Window) => format!("{} [W]", p),
                    (Some(p), SendMode::Global) => p.clone(),
                    // Window mode without a process runs as Global; say so.
                    (None, SendMode::Window) => "Any [W?]".into(),
                    (None, SendMode::Global) => "Any".into(),
                };

                let status_text: String;
                let status_color: Color;
                if active {
                    let progress = state.macro_progress.get(&mac.hotkey_vk);
                    status_text = match (mac.stop_condition, progress) {
                        (StopCondition::AfterReps(max), Some(&(reps, _))) => {
                            format!("{}/{}", reps, max)
                        }
                        (StopCondition::AfterSecs(max), Some(&(_, secs))) => {
                            let remaining = max.saturating_sub(secs);
                            format!("{}s left", remaining)
                        }
                        (_, Some(&(reps, secs))) => {
                            if reps > 0 { format!("x{} {}s", reps, secs) } else { "ON".into() }
                        }
                        _ => "ON".into(),
                    };
                    status_color = GREEN;
                } else {
                    status_text = "---".into();
                    status_color = DIM;
                };

                let row_style = if active {
                    Style::default().fg(Color::Black).bg(BG_ACTIVE_ROW).add_modifier(Modifier::BOLD)
                } else {
                    row_style(selected)
                };

                Row::new(vec![
                    Cell::from(format!("{}{}", cursor, num)),
                    Cell::from(mac.name.as_str()),
                    Cell::from(Span::styled(hotkey, Style::default().fg(ORANGE))),
                    Cell::from(mode),
                    Cell::from(Span::styled(
                        delay,
                        Style::default().fg(if mac.random_delay.is_some() { ORANGE } else { FG_TEXT }),
                    )),
                    Cell::from(Span::styled(
                        stop,
                        Style::default().fg(if mac.stop_condition != StopCondition::None { YELLOW } else { DIM }),
                    )),
                    Cell::from(Span::styled(
                        process,
                        Style::default().fg(if mac.bound_process.is_some() { ORANGE } else { DIM }),
                    )),
                    Cell::from(Span::styled(status_text, Style::default().fg(status_color))),
                ])
                .style(row_style)
            })
            .collect();

        let widths = [
            Constraint::Length(4),
            Constraint::Min(14),
            Constraint::Length(8),
            Constraint::Length(9),
            Constraint::Length(12),
            Constraint::Length(8),
            Constraint::Length(18),
            Constraint::Length(12),
        ];

        let table = Table::new(rows, widths)
            .header(header)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(BORDER_STYLE)
                    .title(Span::styled(
                        if state.renaming {
                            format!(" Rename: {} ", AppState::buf_display(&state.rename_buf, state.text_cursor))
                        } else if state.searching {
                            format!(" Search: {} ", AppState::buf_display(&state.search_query, state.text_cursor))
                        } else if !state.search_query.is_empty() {
                            format!(" Macros ({}/{}) \"{}\" ", filtered.len(), state.macros.len(), state.search_query)
                        } else {
                            format!(" Macros ({}) ", state.macros.len())
                        },
                        if state.renaming || state.searching { Style::default().fg(YELLOW) } else { Style::default().fg(DIM) },
                    ))
                    .title_bottom(status_line),
            );

        let mut table_state = TableState::default().with_selected(Some(state.selected_macro));
        f.render_stateful_widget(table, chunks[1], &mut table_state);
    }

    let button_hint = crate::MAIN_BUTTON_HINTS
        .get(state.selected_button)
        .copied()
        .unwrap_or("");

    let toolbar_block = Block::default()
        .borders(Borders::ALL)
        .border_style(BORDER_STYLE)
        .title(Span::styled(" Actions ", Style::default().fg(DIM)))
        .title_bottom(Line::from(vec![
            Span::styled(format!(" {} ", button_hint), Style::default().fg(DIM)),
        ]))
        .title_bottom(Line::from(vec![
            Span::styled("? ", Style::default().fg(DIM)),
            Span::styled("Help ", Style::default().fg(GRAY)),
        ]).alignment(Alignment::Right));

    let toolbar_inner = toolbar_block.inner(chunks[2]);
    f.render_widget(toolbar_block, chunks[2]);

    if toolbar_inner.height >= 1 {
        f.render_widget(
            Paragraph::new(button_bar_line(MAIN_BUTTONS, state.selected_button)),
            toolbar_inner,
        );
    }
}

fn draw_recording(f: &mut Frame, state: &AppState) {
    let chunks = split_with_status_bar(f, state, &[Constraint::Min(5)]);

    let step_count = state.recording_steps.len();
    let move_count = state.recording_steps.iter()
        .filter(|s| matches!(s.action, KeyAction::MouseMove(_, _)))
        .count();

    let last_keys: String = state
        .recording_steps
        .iter()
        .rev()
        .take(12)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(|s| format!("[{}]", s.action.display_name()))
        .collect::<Vec<_>>()
        .join(" ");

    let total_ms: u64 = state.recording_steps.iter().map(|s| s.delay_ms).sum();
    let elapsed = format!("{:.1}s", total_ms as f64 / 1000.0);

    let status_badge = if state.recording_paused {
        Span::styled(
            "  PAUSED  ",
            Style::default()
                .fg(Color::Black)
                .bg(YELLOW)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(
            "  REC  ",
            Style::default()
                .fg(WHITE)
                .bg(RED)
                .add_modifier(Modifier::BOLD),
        )
    };

    let mouse_badge = if state.recording_mouse_moves {
        Span::styled(
            format!("  MOUSE {}px  ", state.mouse_move_threshold),
            Style::default().fg(Color::Black).bg(GREEN).add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled("  MOUSE OFF  ", Style::default().fg(DIM))
    };

    let content = vec![
        Line::from(""),
        Line::from(vec![Span::raw("  Status:   "), status_badge, Span::raw("  "), mouse_badge]),
        Line::from(""),
        Line::from(if move_count > 0 {
            vec![
                Span::styled("  Steps:    ", Style::default().fg(GRAY)),
                Span::styled(step_count.to_string(), Style::default().fg(WHITE).add_modifier(Modifier::BOLD)),
                Span::styled(format!("  ({} moves, {} actions)", move_count, step_count - move_count), Style::default().fg(DIM)),
            ]
        } else {
            vec![
                Span::styled("  Steps:    ", Style::default().fg(GRAY)),
                Span::styled(step_count.to_string(), Style::default().fg(WHITE).add_modifier(Modifier::BOLD)),
            ]
        }),
        Line::from(vec![
            Span::styled("  Elapsed:  ", Style::default().fg(GRAY)),
            Span::styled(elapsed, Style::default().fg(WHITE)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Keys:     ", Style::default().fg(GRAY)),
            Span::styled(last_keys, Style::default().fg(CYAN)),
        ]),
        Line::from(""),
        // The capture is system-wide, not scoped to this window, and the
        // steps land in the config file as plain text. Someone who starts a
        // quick recording with Alt+Z and then types into another window has
        // no other cue that it is still running.
        Line::from(vec![Span::styled(
            "  Capturing ALL keyboard and mouse input system-wide,",
            Style::default().fg(RED).add_modifier(Modifier::BOLD),
        )]),
        Line::from(vec![Span::styled(
            "  including other windows. Steps are saved to disk as text —",
            Style::default().fg(RED),
        )]),
        Line::from(vec![Span::styled(
            "  press F1 to pause before typing anything private.",
            Style::default().fg(RED),
        )]),
    ];

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(RED))
        .title(Span::styled(
            format!(" Recording: \"{}\" ", state.recording_name),
            Style::default().fg(RED).add_modifier(Modifier::BOLD),
        ))
        .title_bottom(Line::from(vec![
            Span::styled(
                " Esc ",
                Style::default()
                    .fg(WHITE)
                    .bg(RED)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" Stop  ", Style::default().fg(RED)),
            Span::styled(
                " F1 ",
                Style::default()
                    .fg(Color::Black)
                    .bg(YELLOW)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                if state.recording_paused { " Resume " } else { " Pause " },
                Style::default().fg(YELLOW),
            ),
            Span::styled(
                " F2 ",
                Style::default()
                    .fg(Color::Black)
                    .bg(if state.recording_mouse_moves { GREEN } else { GRAY })
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" Mouse  ", Style::default().fg(if state.recording_mouse_moves { GREEN } else { GRAY })),
            Span::styled(
                " F3 ",
                Style::default().fg(DIM).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" {}px ", state.mouse_move_threshold),
                Style::default().fg(DIM),
            ),
        ]));

    f.render_widget(Paragraph::new(content).block(block), chunks[1]);
}

fn config_field_hint(field: ConfigField) -> &'static str {
    match field {
        ConfigField::Name => "Display name for this macro",
        ConfigField::Hotkey => "Enter to bind any key | Esc to cancel",
        ConfigField::RepetitionMode => "Left/Right to cycle: Toggle | Hold | Single",
        ConfigField::DelayMode => "Left/Right to cycle: Recorded | Fixed",
        ConfigField::FixedInterval => "Milliseconds between each step when using Fixed mode",
        ConfigField::RandomDelayMin => "Min ms for random delay (empty = disabled)",
        ConfigField::RandomDelayMax => "Max ms for random delay",
        ConfigField::StopCondition => "Left/Right: None | After N reps | After N seconds",
        ConfigField::StopValue => "Number of repetitions or seconds (depends on stop mode)",
        ConfigField::CycleDelay => "Wait time (ms) between each full cycle of the macro",
        ConfigField::StartDelay => "Wait time (ms) before first execution after activation",
        ConfigField::RequireHeld => "Enter to capture key | Macro pauses when this key is released",
        ConfigField::ExclusiveGroup => "Group name | Activating one stops others in same group",
        ConfigField::ChainMacro => "Enter to select macro to auto-start when this one finishes",
        ConfigField::SendMode => "Global = needs focus (games) | Window = no focus, requires a bound process",
        ConfigField::BoundProcess => "Enter to browse processes | or type process name",
        ConfigField::MouseJitter => "Random pixel offset for MouseMove steps (0 = exact)",
        ConfigField::HumanizeMs => "Random timing jitter per step (±ms). Makes timing less robotic",
    }
}

fn config_field_line(field: ConfigField, state: &AppState, mac: &Macro) -> Line<'static> {
    let is_active = state.config_field == field;
    let (ls, vs) = if is_active {
        (Style::default().fg(YELLOW), Style::default().fg(WHITE).add_modifier(Modifier::BOLD))
    } else {
        (Style::default().fg(GRAY), Style::default().fg(FG_TEXT))
    };
    let arrow = if is_active { ">" } else { " " };

    /// Build the standard two-span config line: label + value.
    fn row(arrow: &str, label: &str, val: String, ls: Style, vs: Style) -> Line<'static> {
        Line::from(vec![
            Span::styled(format!(" {} {:16}", arrow, label), ls),
            Span::styled(val, vs),
        ])
    }

    /// Format the editing buffer with a cursor, or fall back to the display value.
    fn edit_or(is_active: bool, buf: &str, cursor: usize, suffix: &str, display: impl FnOnce() -> String) -> String {
        if is_active {
            format!("{}{}", AppState::buf_display(buf, cursor), suffix)
        } else {
            display()
        }
    }

    fn opt_display(opt: Option<&str>, default: &str) -> String {
        opt.unwrap_or(default).to_string()
    }

    match field {
        ConfigField::Name => {
            row(arrow, "Name:", edit_or(is_active, &state.config_input_buf, state.text_cursor, "", || mac.name.clone()), ls, vs)
        }
        ConfigField::Hotkey => {
            let mut s = vec![Span::styled(format!(" {} {:16}", arrow, "Hotkey:"), ls)];
            if state.awaiting_hotkey {
                s.push(Span::styled("Press a key...", Style::default().fg(YELLOW).add_modifier(Modifier::BOLD)));
            } else if mac.hotkey_vk != 0 {
                s.push(Span::styled("[", Style::default().fg(DIM)));
                s.push(Span::styled(vk_name(mac.hotkey_vk).to_string(), Style::default().fg(ORANGE).add_modifier(Modifier::BOLD)));
                s.push(Span::styled("]", Style::default().fg(DIM)));
            } else {
                s.push(Span::styled("Not set", Style::default().fg(DIM)));
            }
            Line::from(s)
        }
        ConfigField::RepetitionMode => {
            row(arrow, "Repeat mode:", format!("< {} >", mac.repetition.label()), ls, vs)
        }
        ConfigField::StopCondition => {
            let label = match mac.stop_condition {
                StopCondition::None => "< None >",
                StopCondition::AfterReps(_) => "< After N reps >",
                StopCondition::AfterSecs(_) => "< After N secs >",
            };
            row(arrow, "Stop condition:", label.into(), ls, vs)
        }
        ConfigField::StopValue => {
            row(arrow, "Stop value:", edit_or(is_active, &state.config_input_buf, state.text_cursor, "", || mac.stop_condition.display_value()), ls, vs)
        }
        ConfigField::DelayMode => {
            let label = if mac.use_recorded_delays { "< Recorded >" } else { "< Fixed >" };
            row(arrow, "Delay mode:", label.into(), ls, vs)
        }
        ConfigField::FixedInterval => {
            row(arrow, "Fixed interval:", edit_or(is_active, &state.config_input_buf, state.text_cursor, " ms", || format!("{} ms", mac.fixed_interval_ms)), ls, vs)
        }
        ConfigField::RandomDelayMin => {
            row(arrow, "Random min ms:", edit_or(is_active, &state.config_input_buf, state.text_cursor, "", || mac.random_delay.map_or("off".into(), |(m, _)| m.to_string())), ls, vs)
        }
        ConfigField::RandomDelayMax => {
            row(arrow, "Random max ms:", edit_or(is_active, &state.config_input_buf, state.text_cursor, "", || mac.random_delay.map_or("off".into(), |(_, m)| m.to_string())), ls, vs)
        }
        ConfigField::CycleDelay => {
            row(arrow, "Cycle delay:", edit_or(is_active, &state.config_input_buf, state.text_cursor, " ms", || if mac.cycle_delay_ms > 0 { format!("{} ms", mac.cycle_delay_ms) } else { "0".into() }), ls, vs)
        }
        ConfigField::StartDelay => {
            row(arrow, "Start delay:", edit_or(is_active, &state.config_input_buf, state.text_cursor, " ms", || if mac.start_delay_ms > 0 { format!("{} ms", mac.start_delay_ms) } else { "0".into() }), ls, vs)
        }
        ConfigField::RequireHeld => {
            let val = if state.awaiting_require_held { "Press a key...".into() } else if mac.require_held_vk != 0 { format!("[{}]", vk_name(mac.require_held_vk)) } else { "None".into() };
            row(arrow, "Require held:", val, ls, vs)
        }
        ConfigField::ExclusiveGroup => {
            row(arrow, "Exclusive grp:", edit_or(is_active, &state.config_input_buf, state.text_cursor, "", || opt_display(mac.exclusive_group.as_deref(), "None")), ls, vs)
        }
        ConfigField::ChainMacro => {
            row(arrow, "Chain macro:", opt_display(mac.chain_macro.as_deref(), "None"), ls, vs)
        }
        ConfigField::SendMode => {
            row(arrow, "Send mode:", format!("< {} >", mac.send_mode.label()), ls, vs)
        }
        ConfigField::BoundProcess => {
            let val = edit_or(is_active, &state.config_input_buf, state.text_cursor, "", || opt_display(mac.bound_process.as_deref(), "Any (no filter)"));
            let color = if mac.bound_process.is_some() { vs.fg(ORANGE) } else { vs.fg(DIM) };
            row(arrow, "Bound process:", val, ls, color)
        }
        ConfigField::MouseJitter => {
            row(arrow, "Mouse jitter:", edit_or(is_active, &state.config_input_buf, state.text_cursor, " px", || if mac.mouse_jitter > 0 { format!("{} px", mac.mouse_jitter) } else { "0 (exact)".into() }), ls, vs)
        }
        ConfigField::HumanizeMs => {
            row(arrow, "Humanize:", edit_or(is_active, &state.config_input_buf, state.text_cursor, " ms", || if mac.humanize_ms > 0 { format!("\u{00b1}{} ms", mac.humanize_ms) } else { "0 (exact)".into() }), ls, vs)
        }
    }
}

fn draw_config(f: &mut Frame, state: &AppState, idx: usize) {
    let chunks = split_with_status_bar(
        f,
        state,
        &[
            Constraint::Length(3), // tab bar
            Constraint::Min(5),    // config form
            Constraint::Length(3), // help
        ],
    );

    let mac = &state.macros[idx];

    // Tab bar
    let tab_block = Block::default()
        .borders(Borders::ALL)
        .border_style(BORDER_STYLE)
        .title(Span::styled(
            format!(" Configure: \"{}\" ", mac.name),
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ));
    let tab_inner = tab_block.inner(chunks[1]);
    f.render_widget(tab_block, chunks[1]);

    let tab_names: Vec<&str> = crate::state::CONFIG_TABS.iter().map(|(n, _)| *n).collect();
    f.render_widget(
        Paragraph::new(button_bar_line(&tab_names, state.config_tab)),
        tab_inner,
    );

    // Fields for current tab
    let tab_fields = crate::state::CONFIG_TABS[state.config_tab].1;
    let mut content: Vec<Line> = vec![Line::from("")];
    for &field in tab_fields {
        content.push(config_field_line(field, state, mac));
    }
    content.push(Line::from(""));
    content.push(Line::from(vec![
        Span::styled("   Steps: ", Style::default().fg(GRAY)),
        Span::styled(mac.step_count().to_string(), Style::default().fg(WHITE)),
        Span::styled("   Duration: ", Style::default().fg(GRAY)),
        Span::styled(format!("{:.1}s", mac.total_duration_ms() as f64 / 1000.0), Style::default().fg(WHITE)),
    ]));

    let hint_text = config_field_hint(state.config_field);
    let status_line = status_bottom_line(state);

    let form_block = Block::default()
        .borders(Borders::ALL)
        .border_style(BORDER_STYLE)
        .title_bottom(if state.status_message.is_some() {
            status_line
        } else {
            Line::from(Span::styled(format!(" {} ", hint_text), Style::default().fg(DIM)))
        });

    f.render_widget(Paragraph::new(content).block(form_block), chunks[2]);

    let mut spans = Vec::new();
    spans.extend(help_key("Up/Dn", "Fields", ACCENT));
    spans.extend(help_key("Tab", "Section", ACCENT));
    spans.extend(help_key("Enter", "Action", ACCENT));
    spans.extend(help_key("<>", "Cycle", ACCENT));
    spans.extend(help_key("F1", "Steps", ACCENT));
    spans.extend(help_key("F5", "Save", GREEN));
    spans.extend(help_key("Esc", "Cancel", RED));
    render_footer(f, chunks[3], BORDER_STYLE, spans);
}

fn draw_step_editor(f: &mut Frame, state: &AppState, idx: usize) {
    let chunks = split_with_status_bar(f, state, &[Constraint::Min(3), Constraint::Length(3)]);

    let mac = &state.macros[idx];
    let total = mac.step_count();
    let pos = format_pos(state.selected_step, total, "empty");

    let header = Row::new(vec![
        Cell::from("  # "),
        Cell::from("Action"),
        Cell::from("Delay (ms)"),
    ])
    .style(Style::default().fg(ACCENT).add_modifier(Modifier::BOLD));

    let (sel_start, sel_end) = state.step_selection_range(mac.steps.len());

    let rows: Vec<Row> = mac
        .steps
        .iter()
        .enumerate()
        .map(|(i, step)| {
            let selected = i == state.selected_step;
            let in_selection = state.selection_anchor.is_some() && i >= sel_start && i <= sel_end;
            let cursor = if selected { "> " } else if in_selection { "* " } else { "  " };
            let action_display = if selected && state.editing_scroll_clicks {
                format!("Scroll ({})", AppState::buf_display(&state.scroll_clicks_buf, state.text_cursor))
            } else {
                step.action.display_name()
            };
            let delay_display = if selected && state.editing_step_delay {
                AppState::buf_display(&state.step_delay_buf, state.text_cursor)
            } else {
                step.delay_ms.to_string()
            };
            let style = if selected {
                row_style(true)
            } else if in_selection {
                Style::default().fg(WHITE).bg(Color::Rgb(50, 50, 80))
            } else {
                row_style(false)
            };
            let action_color = match &step.action {
                KeyAction::KeyDown(_) | KeyAction::KeyUp(_) => CYAN,
                KeyAction::MouseDown(_) | KeyAction::MouseUp(_) => ACCENT,
                KeyAction::MouseScroll(_, _) => YELLOW,
                KeyAction::MouseMove(_, _) => GREEN,
                KeyAction::TypeText(_) => WHITE,
                KeyAction::WaitForWindow(_) => ORANGE,
            };
            Row::new(vec![
                Cell::from(format!("{}{}", cursor, i + 1)),
                Cell::from(Span::styled(action_display, Style::default().fg(action_color))),
                Cell::from(delay_display),
            ])
            .style(style)
        })
        .collect();

    let widths = [
        Constraint::Length(5),
        Constraint::Min(15),
        Constraint::Length(12),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .row_highlight_style(Style::default()) // we handle highlighting per-row already
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(DIM))
                .title(Span::styled(
                    format!(" Steps: \"{}\" ", mac.name),
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                ))
                .title_bottom(status_or_pos(state, pos)),
        );

    let mut table_state = TableState::default().with_selected(Some(state.selected_step));
    f.render_stateful_widget(table, chunks[1], &mut table_state);

    let mut spans = Vec::new();
    if state.inserting_text {
        spans.push(Span::styled(
            format!(" Text: {}  ", AppState::buf_display(&state.insert_text_buf, state.text_cursor)),
            Style::default().fg(WHITE).add_modifier(Modifier::BOLD),
        ));
        spans.extend(help_key("Enter", "Insert", GREEN));
        spans.extend(help_key("Esc", "Cancel", RED));
    } else if state.editing_step_action {
        spans.extend(help_key("Key/Mouse", "Press to replace", ORANGE));
        spans.extend(help_key("Esc", "Cancel", RED));
    } else if state.editing_step_delay || state.editing_scroll_clicks {
        let label = if state.editing_scroll_clicks { "Scroll clicks" } else { "Delay (ms)" };
        spans.extend(help_key("0-9", label, ACCENT));
        spans.extend(help_key("Enter", "Save", GREEN));
        spans.extend(help_key("Esc", "Cancel", RED));
    } else {
        spans.extend(help_key("t", "Timing", ACCENT));
        spans.extend(help_key("a", "Key", ORANGE));
        spans.extend(help_key("s", "\u{2193}\u{2191}", ACCENT));
        spans.extend(help_key("n", "+Key", GREEN));
        spans.extend(help_key("i", "+Text", GREEN));
        spans.extend(help_key("w/x", "Scroll", GREEN));
        spans.extend(help_key("m", "+Move", GREEN));
        spans.extend(help_key("f", "Wait", CYAN));
        spans.extend(help_key("c", "Dup", ACCENT));
        spans.extend(help_key("y/p", "Cp/Paste", ACCENT));
        spans.extend(help_key("v", "Select", ACCENT));
        spans.extend(help_key("d", "Del", RED));
        spans.extend(help_key("Esc", "Back", YELLOW));
    }
    render_footer(f, chunks[2], Style::default().fg(DIM), spans);
}

/// Shared chrome for the picker views: status bar, bordered table and a
/// footer of help keys.
#[allow(clippy::too_many_arguments)]
fn draw_list_picker(
    f: &mut Frame,
    state: &AppState,
    title: String,
    title_style: Style,
    border_style: Style,
    header: Row,
    widths: &[Constraint],
    rows: Vec<Row>,
    selected: usize,
    pos: String,
    footer: Vec<Span>,
) {
    let chunks = split_with_status_bar(f, state, &[Constraint::Min(3), Constraint::Length(3)]);

    let table = Table::new(rows, widths.to_vec()).header(header).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .title(Span::styled(title, title_style))
            .title_bottom(status_or_pos(state, pos)),
    );

    let mut table_state = TableState::default().with_selected(Some(selected));
    f.render_stateful_widget(table, chunks[1], &mut table_state);

    render_footer(f, chunks[2], border_style, footer);
}

fn draw_process_picker(f: &mut Frame, state: &AppState) {
    let total = state.process_list.len();

    let rows: Vec<Row> = state
        .process_list
        .iter()
        .enumerate()
        .map(|(i, (exe, title))| {
            let selected = i == state.selected_process;
            let cursor = if selected { "> " } else { "  " };
            let title_short: String = title.chars().take(50).collect();
            Row::new(vec![
                Cell::from(Span::styled(
                    format!("{}{}", cursor, exe),
                    Style::default().fg(ORANGE),
                )),
                Cell::from(title_short),
            ])
            .style(row_style(selected))
        })
        .collect();

    let mut footer = Vec::new();
    footer.extend(help_key("Enter", "Select", GREEN));
    footer.extend(help_key("Bksp", "Clear", YELLOW));
    footer.extend(help_key("F5", "Refresh", ACCENT));
    footer.extend(help_key("Esc", "Back", RED));

    draw_list_picker(
        f,
        state,
        format!(" Select Process ({}) ", total),
        Style::default().fg(ORANGE).add_modifier(Modifier::BOLD),
        Style::default().fg(DIM),
        Row::new(vec![Cell::from("  Process"), Cell::from("Window")])
            .style(Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
        &[Constraint::Length(25), Constraint::Min(20)],
        rows,
        state.selected_process,
        format_pos(state.selected_process, total, "no windows"),
        footer,
    );
}

fn draw_chain_picker(f: &mut Frame, state: &AppState) {
    let total = state.chain_list.len();

    let rows: Vec<Row> = state
        .chain_list
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let selected = i == state.selected_chain;
            let cursor = if selected { "> " } else { "  " };
            Row::new(vec![Cell::from(format!("{}{}", cursor, name))]).style(row_style(selected))
        })
        .collect();

    let mut footer = Vec::new();
    footer.extend(help_key("Enter", "Select", GREEN));
    footer.extend(help_key("Bksp", "Clear", YELLOW));
    footer.extend(help_key("Esc", "Back", RED));

    draw_list_picker(
        f,
        state,
        format!(" Chain to ({}) ", total),
        Style::default().fg(DIM),
        BORDER_STYLE,
        Row::new(vec![Cell::from("  Macro name")])
            .style(Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
        &[Constraint::Min(20)],
        rows,
        state.selected_chain,
        format_pos(state.selected_chain, total, "no macros"),
        footer,
    );
}

fn draw_help(f: &mut Frame, state: &mut AppState) {
    let area = f.area();

    // The help text is fully static; rebuilding ~120 lines of spans every
    // frame while the view is open is pure allocation churn.
    static HELP_CONTENT: std::sync::OnceLock<Vec<Line<'static>>> = std::sync::OnceLock::new();
    let content = HELP_CONTENT.get_or_init(build_help_content);
    draw_help_tail(f, state, area, content);
}

fn build_help_content() -> Vec<Line<'static>> {
    let h = |title: &str| -> Line<'static> {
        Line::from(vec![Span::styled(
            format!("  {} ", title),
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        )])
    };

    let s = |key: &str, desc: &str| -> Line<'static> {
        Line::from(vec![
            Span::styled(format!("    {:20}", key), Style::default().fg(WHITE)),
            Span::styled(desc.to_string(), Style::default().fg(GRAY)),
        ])
    };

    let blank = || -> Line<'static> { Line::from("") };

    vec![
        blank(),
        Line::from(vec![Span::styled(
            "  tapmatic — Keyboard & Mouse Macro System",
            Style::default().fg(WHITE).add_modifier(Modifier::BOLD),
        )]),
        blank(),
        Line::from(vec![Span::styled(
            "  Record keyboard and mouse inputs, configure how they replay,",
            Style::default().fg(GRAY),
        )]),
        Line::from(vec![Span::styled(
            "  and bind them to hotkeys for instant activation.",
            Style::default().fg(GRAY),
        )]),
        blank(),
        h("MAIN VIEW"),
        s("Enter", "Activate selected button"),
        s("Space", "Quick edit selected macro"),
        s("Up/Down", "Navigate macro list"),
        s("Left/Right", "Navigate action buttons"),
        s("/", "Search macros by name"),
        s("F2", "Rename macro inline"),
        s("F3", "Cycle sort mode (none/name/hotkey)"),
        s("F12", "Hide window to tray (macros keep running)"),
        s("z", "Undo last action"),
        s("k / j", "Move macro up / down"),
        s("+/-/0", "Speed: slower / faster / reset"),
        s("q", "Quit"),
        s("?", "This help screen"),
        blank(),
        h("RECORDING"),
        s("Esc", "Stop recording and configure"),
        s("F1", "Pause / resume recording"),
        s("F2", "Toggle mouse movement recording"),
        s("F3", "Cycle mouse threshold (1/3/5/10/20 px)"),
        s("Alt+Z", "Start/stop quick record (works without TUI focus)"),
        Line::from(vec![Span::styled(
            "    Keyboard, mouse clicks, and scroll are captured automatically.",
            Style::default().fg(GRAY),
        )]),
        Line::from(vec![Span::styled(
            "    Mouse movement requires F2 to enable (records cursor position).",
            Style::default().fg(GRAY),
        )]),
        Line::from(vec![Span::styled(
            "    F1/F2/F3 work even without TUI focus. Keys held at start are ignored.",
            Style::default().fg(GRAY),
        )]),
        blank(),
        h("MACRO CONFIG (3 tabs: Basic / Timing / Advanced)"),
        s("Up/Down", "Navigate fields in current tab"),
        s("Tab/S-Tab", "Switch between tabs"),
        s("Enter", "Confirm field / bind hotkey / open picker"),
        s("Left/Right", "Cycle values (repeat mode, delay, etc.)"),
        s("F1", "Open step editor"),
        s("F5", "Save and close"),
        s("Esc", "Cancel changes"),
        blank(),
        h("CONFIG FIELDS"),
        s("Name", "Display name for the macro"),
        s("Hotkey", "Key to activate (Enter to capture)"),
        s("Repeat mode", "Toggle (on/off) | Hold (while pressed) | Single (once)"),
        s("Stop condition", "None | After N reps | After N seconds"),
        s("Delay mode", "Recorded (original timing) | Fixed (constant)"),
        s("Random delay", "Min-max ms range for humanized timing"),
        s("Cycle delay", "Wait between each full repetition cycle"),
        s("Start delay", "Countdown before first execution"),
        s("Require held", "Only execute while this key is held"),
        s("Exclusive grp", "Activating one stops others in same group"),
        s("Chain macro", "Auto-start another macro when this finishes"),
        s("Send mode", "Global (needs focus) | Window (PostMessage, no focus; needs a bound process)"),
        s("Bound process", "Only run for specific process window"),
        s("Mouse jitter", "Random pixel offset for mouse moves (anti-detection)"),
        s("Humanize", "Random timing jitter per step (makes timing less robotic)"),
        blank(),
        h("STEP EDITOR"),
        s("t / Enter", "Edit delay (ms) or scroll clicks"),
        s("a", "Replace key (captures new key/mouse button)"),
        s("s", "Swap down/up direction"),
        s("n", "Insert new key step (down+up pair)"),
        s("i", "Insert text step (TypeText)"),
        s("w / x", "Insert scroll up / scroll down step"),
        s("m", "Insert mouse move step (current cursor pos)"),
        s("f", "Insert wait-for-window step (current fg process)"),
        s("c", "Duplicate selected step(s)"),
        s("y", "Copy selected step(s) to clipboard"),
        s("p", "Paste clipboard steps"),
        s("v", "Toggle multi-select mode"),
        s("d", "Delete selected step(s)"),
        s("k / j", "Move step up / down"),
        s("Esc", "Back to config"),
        blank(),
        h("QUICK RECORD"),
        s("Alt+Z", "Start/stop recording from any app"),
        Line::from(vec![Span::styled(
            "    Records a macro without needing the TUI in focus.",
            Style::default().fg(GRAY),
        )]),
        blank(),
        h("SPEED CONTROL"),
        Line::from(vec![Span::styled(
            "    Global speed multiplier affects all macro delays.",
            Style::default().fg(GRAY),
        )]),
        s("+", "Slower (increase delay multiplier)"),
        s("-", "Faster (decrease delay multiplier)"),
        s("0", "Reset to normal speed (x1.0)"),
        blank(),
        h("PERSISTENCE"),
        Line::from(vec![Span::styled(
            "    Settings and macros save to ~/.tapmatic.json automatically.",
            Style::default().fg(GRAY),
        )]),
        Line::from(vec![Span::styled(
            "    Audio, toggle, speed, and all macros are restored on startup.",
            Style::default().fg(GRAY),
        )]),
        blank(),
    ]
}

fn draw_help_tail(f: &mut Frame, state: &mut AppState, area: ratatui::layout::Rect, content: &[Line<'static>]) {
    let visible_height = area.height.saturating_sub(2) as usize;
    let max_scroll = content.len().saturating_sub(visible_height) as u16;
    state.help_scroll = state.help_scroll.min(max_scroll);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(BORDER_STYLE)
        .title(Span::styled(" Help ", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)))
        .title_bottom(Line::from(vec![
            Span::styled(" Esc ", Style::default().fg(DIM).add_modifier(Modifier::BOLD)),
            Span::styled("Close ", Style::default().fg(GRAY)),
            Span::styled("Up/Down ", Style::default().fg(DIM).add_modifier(Modifier::BOLD)),
            Span::styled("Scroll ", Style::default().fg(GRAY)),
        ]));

    f.render_widget(
        Paragraph::new(content.to_vec())
            .block(block)
            .scroll((state.help_scroll, 0)),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::macro_def::{KeyAction, Macro, MacroStep};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn render(state: &mut AppState) {
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal.draw(|f| draw(f, state)).expect("draw");
    }

    fn populated() -> AppState {
        let mut s = AppState::new();
        let mut mac = Macro::new("smoke".into());
        mac.hotkey_vk = 0x41;
        mac.steps.push(MacroStep { action: KeyAction::KeyDown(30), delay_ms: 10 });
        s.macros.push(mac);
        s
    }

    #[test]
    fn indexless_views_render_with_empty_and_populated_state() {
        for view in [View::Main, View::Recording, View::Help] {
            for populate in [false, true] {
                let mut s = if populate { populated() } else { AppState::new() };
                s.view = view.clone();
                render(&mut s);
            }
        }
    }

    #[test]
    fn indexed_views_render_for_an_existing_macro() {
        for view in [
            View::Config(0),
            View::StepEditor(0),
            View::ProcessPicker(0),
            View::ChainPicker(0),
        ] {
            let mut s = populated();
            s.view = view.clone();
            render(&mut s);
        }
    }

    #[test]
    fn step_editor_renders_a_macro_with_no_steps() {
        let mut s = populated();
        s.macros[0].steps.clear();
        s.view = View::StepEditor(0);
        render(&mut s);
    }

    #[test]
    fn main_view_renders_a_filter_with_no_matches() {
        let mut s = populated();
        s.search_query = "no-match".into();
        s.view = View::Main;
        render(&mut s);
    }
}
