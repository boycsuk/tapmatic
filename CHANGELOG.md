# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.3] - 2026-08-24

### Fixed

- Mouse jitter now draws a uniform offset across the configured range. The
  modulo ran on an `i32` cast of the RNG draw, and `%` keeps the sign of its
  dividend, so half the draws were negative and the real range was
  `-2*jitter..=jitter`: a configured ±5 px produced offsets in `-15..=5`,
  three times the intended spread, biased to one side and double-weighted at
  exactly `-jitter`. The arithmetic now stays unsigned through the modulo.
- A `mouse_jitter` above `i32::MAX` no longer overflows the `2*jitter + 1`
  term — a panic in debug builds, a silent wrap in release. The jitter
  computation is 64-bit and saturates, and the destination coordinate is added
  with `saturating_add` so a move near `i32::MAX` cannot wrap to the opposite
  edge of the desktop.
- Values entered in the config editor are clamped at the point of entry rather
  than only when the config is written. Nothing between the editor and the
  executor enforced the ranges, so an out-of-range value was live for the rest
  of the session; `mouse_jitter` was the field where that reached arithmetic
  that could overflow.
- `StopCondition::AfterSecs` is honoured while Window mode retries a vanished
  target. Every other wait in the step loop consults the deadline; this retry
  checked only the stop flag, so a macro whose bound window never came back
  retried indefinitely past its configured limit.

### Changed

- The recording view states that capture is system-wide and that steps are
  written to disk as plain text, and points at `F1` to pause. The previous
  wording ("Mouse and keyboard are captured") did not convey that input from
  other windows is captured too.

### Added

- `CLAUDE.md`, documenting the thread architecture, the invariants that are
  load-bearing across modules (scancode prefixes, `Deadline`, `HeldInputs`
  release-on-drop, the input boundaries) and the known limitations.
- A README section covering what recording captures, that replaying synthetic
  input violates many applications' terms of service, and what `humanize` and
  `mouse_jitter` actually do — and do not do.

## [0.2.2] - 2026-08-24

### Fixed

- Mouse macros now replay correctly on multi-monitor setups. Coordinates are
  recorded in virtual-desktop space, where a monitor left of the primary gives
  negative values, but replay normalized them against the primary monitor's
  size alone, so every click outside it was clamped to the primary's edge and
  landed in the wrong window. Replay now normalizes against the virtual
  desktop's own origin and extent and passes `MOUSEEVENTF_VIRTUALDESK`.
- Arrow keys, the navigation cluster, right Ctrl/Alt and numpad Enter replay as
  themselves instead of their numpad twins. Recording mapped virtual keys with
  `MAPVK_VK_TO_VSC`, which drops the `0xE0` extended prefix, so Up and numpad-8
  both became scancode `0x48` and a recorded arrow typed an `8`. The prefix is
  now kept in the stored scancode and turned back into `KEYEVENTF_EXTENDEDKEY`
  on replay, in both global and window send modes. Macros recorded before this
  release stored those keys without the prefix and are indistinguishable from
  their numpad twins, so an affected step keeps replaying as the numpad key
  until it is re-recorded; hotkeys are unaffected, as they store virtual-key
  codes and are remapped at startup.
- Text inserted in the step editor is truncated before the step is built. The
  truncation was applied to a copy used only for the status message, so an
  oversized paste was written to disk in full and then trimmed by the load-time
  clamp, losing the tail between sessions.
- The step editor refuses inserts past `MAX_STEPS` instead of accepting steps
  that the next launch would silently discard. Duplicate and paste check the
  whole batch up front, so they either fit entirely or change nothing.
- The config file is now flushed to disk before the rename that publishes it,
  and the rename itself is flushed afterwards. Without this a power loss could
  leave a zero-length config, and on a full disk the write appeared to succeed
  while a truncated temp file was renamed over the good config.
- Macros are clamped to their valid ranges on save as well as on load, so what
  is written always matches what will be read back.
- A macro thread that panics now reports itself as finished. Previously the
  event was skipped, leaving the macro shown as ACTIVE and unstoppable, since
  the executor had already dropped its entry.
- `StopCondition::AfterSecs` is honoured during waits and between steps, not
  only between repetitions. A macro parked in `WaitForWindow`, or running one
  long repetition, could overrun its configured limit indefinitely.
- Speed scaling no longer floors a real delay to zero: at 4x every delay under
  4 ms collapsed into a busy send.
- Restarting a recording no longer strands the previous scroll hook. `start` is
  idempotent, and the hook thread publishes its id only after the hook is
  installed and only on success, so a `stop` racing it cannot clear an id a
  newer thread already claimed. Each orphaned `WH_MOUSE_LL` taxed every mouse
  event system-wide.
- Default macro names skip names already in use. Counting the list reused a
  number after a deletion, and since chains resolve their target by name, a
  chain could silently follow the wrong macro.
- Window titles are cut with the length `GetWindowTextW` actually wrote rather
  than the earlier `GetWindowTextLengthW` value, so a title that changes
  between the two calls no longer leaves padding zeros in the string.

### Added

- A DPI manifest declaring `PerMonitorV2` awareness. Without it the process was
  DPI-virtualized and cursor coordinates were reported scaled, so macros
  recorded on a scaled display replayed offset elsewhere.

### Changed

- Interruptible sleeps wake every 25 ms instead of every 5 ms, cutting an
  hour-long delay from 720k wakeups to 144k without a perceptible change in
  stop latency.
- The panic hook installed in `main` was removed; `ratatui::init()` already
  installs one. The second hook could restore the terminal from a macro
  thread's panic while the TUI was still drawing into it.

## [0.2.1] - 2026-08-24

### Added

- A README covering what the app does, how to build and run it, the recording
  and playback keys, and the per-macro options.
- MIT LICENSE file, plus `license`, `description`, `repository` and
  `rust-version` metadata in `Cargo.toml`.

### Changed

- New application icon: an isometric amber block with a graphite top face and
  an inlaid chevron, replacing the grey placeholder hash. The `.ico` now ships
  16/32/48/256 px variants, each rendered at its own scale with an alpha
  channel, so Windows no longer downscales a single 256 px image for the
  taskbar.

### Removed

- `tapmatic.png` — an unreferenced duplicate of the old icon; `app.rc` embeds
  `tapmatic.ico` and nothing read the PNG.

## [0.2.0] - 2026-08-24

### Added

- The step editor's "press a key" capture accepts mouse buttons again, as its
  prompts always claimed: replacing an action or inserting a step pair now
  binds left/middle/right clicks. Hotkey binding stays keyboard-only (a click
  on the UI must not become the macro's trigger), and the X side buttons are
  excluded everywhere - they were capturable as hotkeys and then fired the
  macro on every side-button click anywhere.
- Selecting Window send mode without a bound process now warns in the status
  bar, and the macro table marks the combination with `[W?]`: without a
  process, Window mode silently runs as global focus-dependent input.

### Changed

- Comment and doc hygiene from the audit: three stale doc comments that
  described a different function than the one they sat on are fixed, the
  arm-label and banner comments that restated their code are gone, and the
  items that carry real contracts now have rustdoc - the nine cross-thread
  statics (including `SPEED_MULTIPLIER`'s f64-bits encoding), the view state
  machine and executor command protocol, the serialized config vocabulary
  (`RepetitionMode`'s three modes were documented nowhere), the input
  facade's injection-refused bool, and `ui::draw`.
- Confirmed semantic duplication folded onto single owners: the stop-recording
  transition (QuickRecordToggle now routes through RecordStop), the Config
  entry path (which also inherited a stale tab when opened after a recording),
  the recorder's capture reset, the audio playback scaffold, the insert-after-
  selection rule, the config view's arrow-key motion, and the speed display
  (the status bar and toast could show different precision). Behavioral magic
  numbers got names (mouse VK codes, mode settle window, status TTL, undo cap,
  stop-condition defaults, default mouse threshold).
- Window-mode posts now report `PostMessageW` failures, so the executor's
  retry loop covers a failed post instead of reporting success while sending
  nothing; `get_cursor_pos` failure is now distinguishable from a cursor at
  (0,0), so recording no longer writes a jump-to-origin step when the query
  fails; and the console title is rebuilt on the existing poll cadence
  instead of every frame.

### Security

- CI/release workflows: third-party actions are pinned to commit SHAs (with
  Dependabot proposing bumps), CI's token is now explicitly read-only, the
  release binary is built without a cache so it is always a clean build of
  the tagged commit, and every release now ships a `SHA256SUMS.txt`.

### Fixed

- A macro name (or group/chain/process string) containing control characters
  reached the console title through crossterm's `SetTitle`, which emits a raw
  OSC sequence: a crafted shared config could inject arbitrary terminal
  escape sequences. Control characters are now stripped at the load clamp
  (TypeText payloads excepted - there they are typed input).
- The config file was read and parsed with no size limit, so a multi-gigabyte
  `.tapmatic.json` caused unbounded memory use at every startup. Files over
  64 MB are now refused before parsing and reported as corrupt.
- A failed autosave cleared the dirty flag, so the edits were never retried
  and the quit-path flushes wrote nothing: one transient write failure lost
  the session. The flag now survives failures and the save retries.
- Deleting or renaming a macro left other macros' `chain_macro` references
  dangling, silently breaking their chains; a chain to a missing name also
  failed with no feedback. Renames now retarget references, deletes clear
  them (reporting how many), and a dangling chain is named in the status bar.
- A pending delete confirmation survived a quick recording that changed the
  macro list, so Enter deleted a different macro than the prompt named; the
  commit also bypassed an active search filter, desyncing the selection.
  Committing a recording now cancels the prompt and clears the filter.
- Starting a macro when the executor thread was dead still marked it ACTIVE
  and played the activation sound, misreporting forever.
- The legacy-migration guard against overwriting a config that appeared
  mid-migration used a non-atomic exists-then-rename; the target is now
  reserved atomically.
- The first run (or a legacy migration) now reports itself in the status bar
  instead of being indistinguishable from a normal load.
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
