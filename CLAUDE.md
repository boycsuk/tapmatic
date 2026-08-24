# CLAUDE.md

Project conventions and non-obvious constraints for tapmatic. Read this before
changing anything under `src/`.

## What this is

A Windows macro recorder and player: capture real keyboard and mouse input,
replay it on a hotkey. One binary, a ratatui TUI, no installer, no runtime
dependencies beyond the OS.

The crate compiles on Linux so the test suite can run in CI, but every input
path is a no-op stub there. A green Linux run proves the logic compiles and the
pure functions behave; it proves nothing about real input. **The Windows CI leg
is the one that matters** — it is where the `unsafe` Win32 code is built.

## Architecture

Four threads, communicating over `mpsc` channels. No shared mutable state
except the atomics in `main.rs`.

- **main** — owns `AppState`, runs the TUI draw loop, dispatches `AppEvent`.
  Everything that mutates `AppState` happens here, single-threaded.
- **hotkey** (`hotkey.rs`) — polls `GetAsyncKeyState` on an interval, emits
  `AppEvent`. Handles both hotkey firing and recording capture.
- **executor** (`executor.rs`) — owns running macros, one thread per active
  macro. Receives `ExecutorCommand`, sends `AppEvent` back.
- **scroll-hook** (`input.rs`) — only alive during a recording. A
  `WH_MOUSE_LL` hook needs its own message pump, so it gets its own thread.

`ui.rs` is pure rendering: it reads `AppState` and draws. It must not mutate.

`unsafe` is confined to `input.rs`. Keep it that way — if a new feature needs a
Win32 call, wrap it in a safe function there rather than spreading `unsafe`
across modules.

## Invariants

These are load-bearing. Breaking one produces a bug that is hard to trace back.

**Scancodes carry an extended prefix.** A recorded scancode is not a plain
hardware code: extended keys (arrows, the navigation cluster, right Ctrl/Alt,
numpad Enter) store `0xE0` in the high byte, because they share their low byte
with a numpad key. `split_scancode` is the only correct way to take one apart;
mapping must use `MAPVK_VK_TO_VSC_EX`, never `MAPVK_VK_TO_VSC`. Getting this
wrong makes a recorded Up arrow type an `8`.

**`Deadline` must be consulted by every wait.** `StopCondition::AfterSecs` is
the user's promise that a macro ends on time. Any loop that can block — a
sleep, a poll, a retry — has to check `deadline.expired()` alongside
`should_stop`. A loop that only checks the stop flag runs forever when the
thing it waits for never happens.

**`HeldInputs` releases on `Drop`, not on the normal path.** A macro that
panics or is stopped mid-step must not leave keys physically pressed
system-wide. The release therefore lives in `Drop`, and it must use the same
channel the presses used — releasing a Window-mode press through the global
path injects key-ups into whatever window happens to have focus.

**`FinishedOnDrop` fires exactly once, on unwind too.** The UI marks a macro
active when it starts; without a guaranteed `MacroFinished` a panicking macro
shows as ACTIVE forever and cannot be stopped, since the executor already
dropped its entry.

**The config file is the untrusted-input boundary.** Macro files get shared
between users. `clamp_to_valid_ranges` is the only thing between `serde_json`
and the executor, and it runs on load *and* on save — on save so that what is
written equals what will be read back, on load so a hand-edited file cannot
reach the executor out of range.

**The editor is an input boundary too.** `ConfigField::write` clamps after
writing. Clamping only at save time leaves an out-of-range value live for the
whole session, which is how a `mouse_jitter` past `i32::MAX` used to reach the
executor's arithmetic and overflow it.

**A disabled feature must not advance the RNG.** `humanize` and `mouse_jitter`
are gated at the call site, not just inside their helpers. Drawing a random
number for a feature that is off changes the sequence every other consumer
sees, so toggling an unrelated setting would change a macro's timing.

**Randomness goes through `u64` before it is narrowed.** `%` in Rust keeps the
sign of its dividend, so casting an RNG draw to a signed type before the modulo
yields a range skewed to the negative side. See `jitter_offset` — this was a
real bug: a configured ±5 px produced offsets in `-15..=5`.

## Conventions

- Comments explain **why**, never what. The codebase is consistent about this;
  match it. A comment that paraphrases its line should not exist.
- Doc comments (`///`) on public items are the API contract and do describe
  what — that is the deliberate exception.
- Every fix gets a regression test that fails before it and passes after.
- Constants get names. `MAX_DELAY_MS`, `WINDOW_RETRY_POLL_MS`, not literals.
- English everywhere in the repo: code, comments, commits, docs.

## Verification

```bash
cargo test
cargo clippy --all-targets -- -D warnings
```

Both must be clean before a commit. CI runs them on Linux and Windows.

Tests must not inject real input: a `KeyDown` step in a test would type into
the CI runner. Use an empty `TypeText`, which sends nothing on any platform —
see `instant_macro` in `executor.rs`.

Audio is disabled in tests (`AUDIO_ENABLED.store(false)`) so real playback
threads do not outlive the run.

## Known limitations

- **Macros recorded before 0.2.2 store extended keys without the prefix**, so
  those steps are indistinguishable from their numpad twins and keep replaying
  as the numpad key until re-recorded. Not fixable retroactively — the
  information is gone.
- **`SendInput` cannot reach an elevated window** from a non-elevated process
  (UIPI), nor the secure desktop. `send` returns the injected count and the
  executor warns once, but there is no way around it short of elevating.
- **Window mode does not support scroll or move.** `PostMessage` handles
  `WM_MOUSEWHEEL`/`WM_MOUSEMOVE` unreliably, so those two actions fall back to
  global `SendInput` even in Window mode, and go to the focused window.
- **HWND values are recycled by Windows.** A cached handle can come to belong
  to a different program, so ownership is re-verified on a coarse interval
  (`IDENTITY_RECHECK_INTERVAL`). Between checks a send can in principle reach
  the wrong window.
- **Recording captures system-wide, in plain text.** Anything typed during a
  recording lands in `.tapmatic.json`. The file is `0600` on Unix; on Windows
  it inherits the profile directory's ACL. This is inherent to what a macro
  recorder does, not a defect, but it is a real privacy exposure — the
  recording view warns about it and `F1` pauses capture.
- **`humanize_ms` and `mouse_jitter` do not defeat automation checks** and
  should not be documented as if they do. `SendInput` flags its events as
  injected (`LLMHF_INJECTED` is visible to any low-level hook), which no amount
  of timing jitter changes. They make replay less mechanical; that is all.
