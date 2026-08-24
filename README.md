# tapmatic

Record keyboard and mouse input on Windows, then replay it on a hotkey. A
terminal UI, one binary, no installer.

Macro tools are usually either a scripting language you have to learn or a GUI
that hides what it is doing. tapmatic records what you actually did, shows every
step as an editable list, and puts the timing controls where you can see them.

## Quickstart

```bash
git clone https://github.com/boycsuk/tapmatic
cd tapmatic
cargo build --release
./target/release/tapmatic.exe
```

Press `Enter` on **Record**, do the thing you want repeated, press `Esc`. Bind a
hotkey in the config screen, press `F5`, and the key you bound now replays it.

Prebuilt binaries are on the [releases page](https://github.com/boycsuk/tapmatic/releases)
if you would rather not build it.

## Requirements

- Windows. The input layer is Win32 (`SendInput`, `PostMessage`,
  `GetAsyncKeyState`); on other platforms the crate compiles but no macro runs.
- Rust 1.87+ to build from source.

## Usage

**Recording.** `Alt+Z` starts and stops a quick recording from any window, so
you do not need the TUI focused. `F2` during a recording toggles mouse-movement
capture, and `F3` cycles how far the cursor must travel before a move is
recorded (1/3/5/10/20 px).

**Playing back.** Each macro has a repeat mode: toggle on and off with the
hotkey, run only while the key is held, or fire once. A stop condition can end
it after N repetitions or N seconds. `+` and `-` scale the delays of everything
currently running; `0` resets to normal speed.

**Editing steps.** `F1` from the config screen opens the step list, where every
key press, click, scroll and pause is a row you can retime, replace, duplicate
or delete. `i` inserts typed text, `m` inserts a mouse move at the current
cursor position, and `f` inserts a wait until a given window is in the
foreground.

**Targeting a window.** Global send mode uses `SendInput`, which goes to
whatever window has focus. Window mode posts messages to one specific process
instead, so a macro can run while you work elsewhere — it needs a bound process,
and the app says so if you forget one.

Press `?` in the app for the full key reference.

## Configuration

Macros live in `.tapmatic.json` in your home directory. The file is written
atomically and re-read at startup; there are no settings outside it.

Per-macro options whose names do not give them away:

| Option | Meaning |
| --- | --- |
| Require held | Only advance while this key is physically held down |
| Exclusive group | Starting one macro stops any other in the same group |
| Chain macro | Start another macro when this one finishes naturally |
| Random delay | Draw each pause from a min-max range instead of a fixed value |
| Humanize | Add random jitter to every step's timing |
| Mouse jitter | Offset mouse moves by a few random pixels |

## Contributing

```bash
cargo test
cargo clippy --all-targets -- -D warnings
```

CI runs both on Linux and Windows. The Windows job is the one that compiles the
`unsafe` Win32 code, so check it before assuming a change is fine.

## License

MIT — see [LICENSE](LICENSE).
