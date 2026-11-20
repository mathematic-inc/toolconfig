# Changelog

## 2026.11.5 (unreleased)

### Bug fixes

- ryl's rules go under `lint.rules`, where ryl 0.25 and later read them. ryl ignored the
  generated top-level `rules` when a source also set `ryl.lint.rules`, and warned that `rules`
  is deprecated.
- In a Pkl source, an entry that holds only elements, such as
  `["license-files"] { new { path = "LICENSE" } }`, now becomes an array. It was an empty table.
  ToolConfig now embeds pklr 6.0.3.

### Changes

- ryl has no `yaml` format, because ryl deprecates yamllint YAML configuration. Remove
  `format = "yaml"` from `[toolconfig.ryl]` to write `.ryl.toml`.

## 2026.11.4 (2026-11-20)

### Changes

- Ownership records move from `.toolconfig-state/state.json` to `.toolconfig.lock`, a TOML file
  beside the root source. The first `generate`, `clean` or `init` moves existing records and
  removes `.toolconfig-state`; commit both changes. Read-only commands still understand the old
  layout until then.
- The writer lock and the publication journal are machine-local, so they now live outside the
  project under the user's state directory. `TOOLCONFIG_STATE_DIR` overrides the location.

## 2026.11.3 (2026-11-10)

### Features

- Vale joins the catalog (`.vale.ini`). A style and a vocabulary are plain files. The README shows
  how a shared Pkl source writes them with `files`.
- INI files take lists, as one comma-separated entry. ToolConfig writes a comma inside an item as
  `\,`, which Vale's TokenIgnores and BlockIgnores read as a comma. sqlfluff and Vale's other lists
  split at every comma.

## 2026.11.2 (2026-11-08)

### Features

- Inherit complete TOML or Pkl configurations from public and private Git repositories pinned
  to full commit IDs. Local declarations override imported settings. Fetching uses Git's
  credential helpers and SSH configuration, with verified snapshots cached for reuse.

## 2026.11.0 (2026-11-02)

### Features

- golangci-lint (`.golangci.yml`, v2), hadolint (`.hadolint.yaml`), tflint (`.tflint.hcl`, or
  `.tflint.json`) and shellcheck (`.shellcheckrc`) join the catalog.
- golangci-lint takes the width as `lll`'s `line-length`, only where its own options turn `lll`
  on.
- tflint's file is written as HCL, laid out as `hclfmt` lays it out. An object tflint's schema
  doesn't place is refused, naming `tflint.format = "json"` as the fix.
- In a TOML section, `shellcheck.*` keys are now shellcheck's options. Quote the key to write it
  to `.editorconfig` instead.

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
