# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed

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

- Macro fields are validated and clamped when loaded, bounding virtual-key
  codes, delays, step counts and `TypeText` lengths at the untrusted-input
  boundary.
- Tests for the persistence layer, the executor's delay arithmetic, the
  recorder's event filtering and `calc_cpu_percent`, none of which had any.

### Changed

- Corrected doc comments that described behaviour the code does not have:
  `speed_multiplier` documented the reciprocal of what it stores, `config_path`
  claimed the config is always in the home directory, and the platform note in
  `input.rs` understated the non-Windows stubs.
- Removed four unused `windows` crate features and two write-only `AppState`
  fields.
