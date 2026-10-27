# Changelog

## 2026.10.2 (2026-10-27)

### Features

- Generated tool files are gitignored by default: `init` adds the managed `.gitignore` block to
  a new source, and `--no-gitignore` commits them instead. `.editorconfig` and the state file
  stay committed.
- `check` accepts ignored files that are missing, as in a fresh clone, and still reports ignored
  files that are stale or edited.

## 2026.10.1 (2026-10-27)

### Bug fixes

- Watch mode no longer regenerates when a project-local Pkl package cache changes. On Windows,
  activity inside the cache is reported as a change to the cache directory itself.
- Watch mode ignores metadata-only changes and events replayed from before it started, and
  regenerates for a file it already read only when the file's content changes.
- `TOOLCONFIG_WATCH_TRACE` prints what triggered a regeneration.

## 2026.10.0 (2026-10-27)

### Features

- Generate `.editorconfig` and native configuration for 22 tools from TOML sources
  (`.toolconfig`, `.toolconfig.toml`, `.editorconfig.toml`) or Pkl sources (`.toolconfig.pkl`).
- Project indentation, line width, line endings, final newlines and spelling language into each
  tool's native keys, with per-extension overrides for prettier and oxfmt.
- Detect conflicting values and sections that a native file cannot follow, with fixes in the
  error message.
- `init`, `generate` (with `--watch`), `check`, `validate`, `list`, `explain` and `clean`.
- Journaled, ownership-checked publication with a committed, machine-independent state file.
- A JSON Schema for TOML sources and a Pkl package for Pkl sources.
