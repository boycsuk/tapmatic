# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed

- A macro name (or group/chain/process string) containing control characters
  reached the console title through crossterm's `SetTitle`, which emits a raw
  OSC sequence: a crafted shared config could inject arbitrary terminal
  escape sequences. Control characters are now stripped at the load clamp
  (TypeText payloads excepted - there they are typed input).
- The config file was read and parsed with no size limit, so a multi-gigabyte
  `.tapmatic.json` caused unbounded memory use at every startup. Files over
  64 MB are now refused before parsing and reported as corrupt.
- In-session edits bypassed every load-time bound: a huge mouse-jitter or
  scroll-clicks value overflowed the executor's i32 math (garbage input
  injected system-wide in release builds), and oversized names, TypeText
  payloads, recordings and macro counts were silently truncated at the next
  launch. Config commits now run the same clamp as the load path, the
  recorder stops at the step cap, and copy/record refuse to exceed the macro
  cap. The wheel-delta math also saturates as defense in depth.
- A macro with zero steps was startable and became an unstoppable busy loop:
  no iteration ever read the stop flag, `stop()` joined forever and quitting
  hung the app. Empty macros are now refused at start and the executor loop
  checks the stop flag every cycle.
- Copying or duplicating steps with a selection anchor left over from a
  longer macro panicked: the anchor was never reset when entering the step
  editor and the selection range was not clamped to the step list.
- Opening a macro's config with a text cursor left over from a longer buffer
  (e.g. the search field) panicked on Backspace or Ctrl+Left: `enter_config`
  never reset the cursor.

## [0.1.0] - 2026-08-23

### Fixed

- Loading a config whose macro name or `TypeText` fell on a multibyte char
  boundary panicked at every startup: the clamp used byte-index
  `String::truncate`. The app itself could write such a name, since the input
  path caps nothing. Truncation now backs up to a char boundary.
- The load-time clamp missed several fields: `MouseScroll` clicks (i32
  overflow when scaled by `WHEEL_DELTA`), `MouseMove` coordinates (overflow
  when jittered), `WaitForWindow`, `bound_process`, `exclusive_group` and
  `chain_macro` strings, and the macro count itself.
- Quitting with a macro mid-keypress left keys held system-wide: the quit
  path dropped `RUNNING` before `StopAll` and never joined the executor, so
  process exit killed the macro threads before they released anything. Held
  inputs are now also released via `Drop`, so a panic in the step loop can no
  longer skip the release.
- Stopping a Window-mode macro released its keys through global `SendInput`
  instead of the target window: the target kept a key logically held while
  the focused window received spurious key-ups.
- An aborted macro still fired its chain, so a self-chaining macro restarted
  forever with no way to stop it — the kill switch included. Chain starts now
  require a natural finish and obey the same gates as hotkey starts.
- A macro thread that failed to spawn was reported as running forever.
- The macro RNG was seeded from an `Instant` created moments earlier, so
  every run produced the same "random" delay, humanize and jitter sequence —
  the opposite of what those features exist for. Seeded from the system clock
  now.
- `TypeText` slept 5 ms per character with no stop check: a full-length text
  blocked stop, quit and the executor loop for up to ~50 seconds. Typing now
  aborts when the macro is stopped.
- The legacy migration read `macros.json` from the working directory, so
  launching the binary in any directory containing one silently adopted its
  macros and deleted the file. Only the executable's own directory is
  searched now, and the migration refuses to overwrite a config that
  appeared in the meantime.
- The config search honoured `USERPROFILE` over `HOME` on Unix and fell back
  to the working directory when neither was set; the fallback is now the
  executable's directory, gated per platform.
- The save temp file followed symlinks, was world-readable and was left
  behind when the rename failed. It is now created fresh with owner-only
  permissions (macros can hold typed text) and cleaned up on failure.
- Stopping a recording could race the scroll-hook thread's startup and leak
  the system-wide mouse hook (plus its thread) for the rest of the session,
  with later recordings receiving duplicate scroll events. `start` now waits
  until the thread is reachable, and a failed hook install reports itself in
  the status bar instead of panicking the thread.
- Clicking during "press a key" hotkey capture bound the macro to a mouse
  button, making it fire on every click anywhere.
- A poisoned hotkey-config lock silently disabled all hotkeys (and hotkey
  updates) for the rest of the session.
- Window-mode targeting validated its cached handle with `IsWindow` only;
  Windows recycles handle values, so a target that closed mid-run could
  leave the macro posting keystrokes into an unrelated program. Ownership is
  now re-verified against the bound process every 500 ms.

- Deleting a macro while a search filter was active removed a different macro
  than the highlighted one, and the confirmation prompt named the wrong macro.
  `selected_macro` indexes the filtered list, but the delete path used it as a
  raw index into `macros`.
- Cursor navigation and clamping bounded the selection against the full macro
  list instead of the visible rows, letting the cursor leave the visible list
  while a filter was active.
- The legacy `macros.json` migration deleted the old file even when writing the
  new config failed, destroying the only copy of the user's macros.
- Saving wrote the config in place, so an interrupted write truncated it. It is
  now written to a temporary file and renamed over the target.
- A corrupt or unreadable config silently loaded as an empty default and was
  then overwritten by the next autosave. Saving is now disabled for the session
  and the failure is reported.
- Save failures were discarded while the UI reported success unconditionally.
- A macro with an out-of-range `random_delay` triggered a divide-by-zero panic
  that killed the macro thread without releasing held keys.
- `calc_cpu_percent` could underflow on inconsistent counter snapshots.
- The F3 sort discarded manual macro ordering with no undo snapshot and without
  persisting the result.
- `speed_multiplier` was not clamped when loaded from disk.

### Added

- Regression tests for the previously fixed bugs that had none: deleting
  under an active search filter, the F3 sort's undo snapshot and persistence,
  and the whole persistence layer through its real `load`/`save`/migration
  entry points. New behavior tests cover hotkey toggle semantics, exclusive
  groups, chain gating, the recording view gate, and the executor's
  `AfterReps` stop condition.
- The hotkey edge-detection logic is extracted into pure functions
  (previously it was welded to `GetAsyncKeyState`, untestable on any
  platform) and covered by an edge-table test suite.
- Rendering smoke tests for every view (empty and populated state, empty
  step list, filter with no matches) — `ui.rs` had no tests at all; recorder
  pause-resume and drain tests; flush success/failure tests against a
  temporary `HOME`; and two Windows-only tests exercised by the Windows CI
  leg, which previously asserted nothing platform-specific.
- Macro fields are validated and clamped when loaded, bounding virtual-key
  codes, delays, step counts and `TypeText` lengths at the untrusted-input
  boundary.
- Tests for the persistence layer, the executor's delay arithmetic, the
  recorder's event filtering, `calc_cpu_percent` and the step editor's key
  handling, none of which had any.
- CI workflow building and linting on Linux and Windows. The Windows-only half
  of `input.rs` and `audio.rs` was never compiled by a Linux-only build.

- `SendInput` return values were ignored at every call site, so a macro
  blocked by UIPI or a secure desktop "ran" while sending nothing. Injection
  failure now surfaces a one-shot status-bar warning per run.

### Performance

- The console title is rebuilt and written only when it changes, instead of
  every frame; the fully static help view content is built once instead of
  per frame.
- A Global-mode macro bound to a process queried the foreground window on
  every step (~100 syscall clusters/second at 10 ms intervals); the answer
  is now cached for 75 ms. The hotkey scanner no longer clones the bound-key
  list on every 10 ms poll.
- Config writes no longer happen synchronously inside key handlers; changes are
  flushed at most once per second and on quit.
- The foreground process is polled ~4 times per second rather than every frame.
- In Window mode the target window is resolved once per macro and revalidated,
  instead of a full `EnumWindows` sweep for every step.

### Changed

- Extracted the repeated modal text-input pattern and the shared picker chrome;
  `handle_key_step_editor` drops from 290 to 182 lines.
- Toolbar dispatch uses named button constants instead of bare integers.
- Removed 48 decorative section banners and comments that restated the code.
- Corrected doc comments that described behaviour the code does not have:
  `speed_multiplier` documented the reciprocal of what it stores, `config_path`
  claimed the config is always in the home directory, and the platform note in
  `input.rs` understated the non-Windows stubs.
- Removed four unused `windows` crate features and two write-only `AppState`
  fields.
- Removed the leftovers the previous cleanup missed: the empty
  `handle_key_recording` no-op, an orphaned `// Stats` banner, ~25 comments
  restating the line below them, and three over-visible `pub` items. Fixed
  the doc comments that had drifted from the code (a doc attached to the
  wrong function in `input.rs`, a stale paragraph on `autosave` from its
  synchronous era, and two smaller inaccuracies), and added one-line rustdoc
  to the core public items (`AppState`, `Macro`, `KeyAction`, `AppEvent`,
  `hotkey_loop`, `executor_loop`, `HotkeyConfig`).
