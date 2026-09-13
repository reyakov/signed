# TODO

## `BackendEvent::SyncProgress`

File: `crates/signed_state/src/backend.rs`

Kept intentionally. The progress pipeline (the `SyncProgress` variant, the `sync_progress`
field and its accessor, and the progress task in `sync_bootstrap`) is retained for a planned
sync progress indicator. No subscriber exists yet. Do not remove it without revisiting that
plan.

## `login` / `logout` family

File: `crates/signed_state/src/backend.rs`

No UI path calls these. `import_dialog::open` is an empty stub. Decide whether to
delete the family (`login`, `login_with_new_identity`, `login_with_nsec`,
`login_with_bunker`, `logout`) or wire the stub to `Backend::login`.
