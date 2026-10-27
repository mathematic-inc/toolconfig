# Changelog

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
