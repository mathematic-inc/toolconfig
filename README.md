# ToolConfig

[![crates.io](https://img.shields.io/crates/v/toolconfig?style=flat-square)](https://crates.io/crates/toolconfig)
[![license](https://img.shields.io/crates/l/toolconfig?style=flat-square)](LICENSE)
[![ci](https://img.shields.io/github/actions/workflow/status/mathematic-inc/toolconfig/ci.yml?branch=main&label=ci&style=flat-square)](https://github.com/mathematic-inc/toolconfig/actions/workflows/ci.yml)

EditorConfig, extended to your tools.

You write EditorConfig sections in TOML, plus a table of native options for each tool.
ToolConfig generates the plain `.editorconfig` that editors read and each tool's own
configuration file: `rustfmt.toml`, `ruff.toml`, `.prettierrc.json`, and so on. Indentation,
line width and line endings are written once, and every tool receives them in its own keys,
so editors, formatters and linters cannot disagree.

```toml
# .toolconfig.toml
root = true

[toolconfig]
tools = ["rustfmt", "prettier", "ruff"]

["*"]
end_of_line = "lf"
insert_final_newline = true
indent_style = "space"
indent_size = 2
max_line_length = 100

["*.{rs,py,pyi}"]
indent_size = 4

["*.rs"]
rustfmt.edition = "2024"

["*.md"]
max_line_length = 80
prettier.proseWrap = "always"

["*.{py,pyi}".ruff.lint]
select = ["E", "F", "I"]
```

`toolconfig generate` writes `.editorconfig` for editors and these native files:

```toml
# rustfmt.toml
hard_tabs = false
tab_spaces = 4
max_width = 100
newline_style = "Unix"
edition = "2024"
```

```json
// .prettierrc.json
{
  "useTabs": false,
  "tabWidth": 2,
  "printWidth": 100,
  "endOfLine": "lf",
  "overrides": [
    {
      "files": ["**/*.md"],
      "options": {
        "printWidth": 80,
        "proseWrap": "always"
      }
    }
  ]
}
```

```toml
# ruff.toml
indent-width = 4
line-length = 100

[format]
indent-style = "space"
line-ending = "lf"

[lint]
select = ["E", "F", "I"]
```

Tools and editor extensions read these files through their normal discovery, so nothing else
changes. Commit the generated files, including `.toolconfig-state/state.json`, and run
`toolconfig check` in CI.

## Installation

Download an archive from the [releases](https://github.com/mathematic-inc/toolconfig/releases),
or install a prebuilt binary with `cargo binstall toolconfig` or
`mise use github:mathematic-inc/toolconfig`. To build from source, run
`cargo install toolconfig --locked` with Rust 1.98 or newer.

## Sources

Each directory may hold one source. Every name below uses the same model, and directories can
mix them.

| Filename | Syntax | Notes |
| --- | --- | --- |
| `.toolconfig.toml` | TOML | What `toolconfig init` creates. Editors recognize the extension |
| `.toolconfig` | TOML | Named like `.editorconfig`. Map it to TOML in your editor for highlighting |
| `.editorconfig.toml` | TOML | Names itself as the source of the `.editorconfig` beside it |
| `.toolconfig.pkl` | Pkl | Functions, imports and types, evaluated by an embedded Pkl interpreter |

The project root is the nearest directory whose source sets `root = true`; `--root DIR` or
`TOOLCONFIG_ROOT` overrides the search. Every source below the root takes part, except in
Git-ignored paths and in subdirectories that start their own project with `root = true`.
Sources cascade exactly like `.editorconfig` files: closer sources win, and later sections win
within a source.

### TOML sources

- **`root = true`** marks the project root. It must appear in the root source.
- **`[toolconfig]`** configures the files generated in this directory:

  | Key | Meaning |
  | --- | --- |
  | `tools` | Tools whose native file is generated here, such as `["rustfmt", "prettier"]` |
  | `<tool>.format` | One of the tool's native formats, such as `"yaml"` for prettier |
  | `<tool>.path` | One of the filenames the tool reads, such as `"_typos.toml"` |
  | `<tool>.skip` | Standard properties not projected into the tool; `["*"]` skips all |
  | `files.<label>` | A custom output with `path`, `format` and `config` |

- **`["<glob>"]`** is an EditorConfig section. The quotes are required. Inside a section:
  - Standard properties (`indent_style`, `indent_size`, `tab_width`, `end_of_line`, `charset`,
    `trim_trailing_whitespace`, `insert_final_newline`, `max_line_length`,
    `spelling_language`) are typed, written to `.editorconfig` and projected into tools.
  - A tool's id (`rustfmt.edition`, `["*.py".ruff.lint]`) holds native options for that tool.
  - Anything else, such as `shell_variant = "bash"` or
    `dotnet_diagnostic.CA1822.severity = "warning"`, is written to `.editorconfig` as is.
- Keep each section's tables together. Section order decides precedence, as in EditorConfig.

Add `#:schema https://github.com/mathematic-inc/toolconfig/releases/download/v2026.10.0/toolconfig.schema.json`
at the top for completion in Taplo, Tombi and other TOML editors.

### Pkl sources

```pkl
amends "package://github.com/mathematic-inc/toolconfig/releases/download/v2026.10.0/toolconfig@2026.10.0#/ToolConfig.pkl"

local width = 100

root = true

toolconfig {
  tools = List("rustfmt")
}

sections {
  ["*"] {
    indent_size = 2
    max_line_length = width
  }
  ["*.rs"] {
    indent_size = 4
    rustfmt {
      edition = "2024"
    }
  }
  ["*.sh"] {
    properties {
      ["shell_variant"] = "bash"
    }
  }
}
```

The package is embedded in the binary, so evaluation works offline. Other EditorConfig
properties go in `properties`. Pkl can read `env:TOOLCONFIG_ROOT_DIR` and
`env:TOOLCONFIG_SOURCE_DIR`.

## How values reach each tool

ToolConfig resolves the sections for one probe file per extension a tool handles, in the
directory that enables it. Values that agree go at the top level of the native file. Values
that differ by extension go into native `overrides` when the tool has them (prettier, oxfmt);
otherwise generation fails and says which files disagree. A file without a value gets the
tool's default.

Sections that single out some files, such as `["src/legacy/**.rs"]` or `["**/test_*.py"]`, are
checked with a concrete path from their glob. When they would change what a tool does, the
error suggests listing the tool in a source in that directory, which gives it its own native
file, or skipping the property.

Setting the same native key twice is an error:

```text
rustfmt.max_width is set twice for *.rs files
  .toolconfig.toml:7 ["*.rs"] rustfmt.max_width = 120
  .toolconfig.toml:5 ["*"] max_line_length = 100
help: change one of them, or add the property to [toolconfig.rustfmt] skip in .toolconfig.toml
```

Generated files follow the EditorConfig properties at their own path, so formatters that read
the same properties leave them unchanged.

| Tool | Properties it receives | Native file |
| --- | --- | --- |
| csharpier | indent, width, line ending | `.csharpierrc.yaml` |
| oxfmt | indent, width, line ending, final newline | `.oxfmtrc.json` |
| prettier | indent, width, line ending | `.prettierrc.json` |
| ruff | indent, width, line ending | `ruff.toml` |
| rumdl | width | `.rumdl.toml` |
| rustfmt | indent, width, line ending | `rustfmt.toml` |
| ryl, yamllint | indent, width, line ending | `.ryl.toml`, `.yamllint.yaml` |
| sqlfluff | indent, width | `.sqlfluff` |
| tombi | indent, width, line ending | `tombi.toml` |
| typos | spelling language | `typos.toml` |
| yamlfmt | indent, width, line ending | `.yamlfmt` |
| actionlint, buf, cargo-deny, editorconfig-checker, gitleaks, knip, lychee, oxlint, ty, typespec | native options only | their usual file |

shfmt and ktlint read `.editorconfig` themselves; write their keys in sections.

## Commands

```text
toolconfig [--root DIR] init [--format toml|pkl] [--gitignore]
toolconfig [--root DIR] generate [--watch] [--strict]
toolconfig [--root DIR] check [--strict]
toolconfig [--root DIR] validate [--strict]
toolconfig [--root DIR] list
toolconfig [--root DIR] explain [TOOL] [--dir DIR]
toolconfig [--root DIR] clean
```

| Command | Effect |
| --- | --- |
| `init` | Create a root source, preserving an existing one. `--gitignore` adds a managed block for generated files other than `.editorconfig` |
| `generate` | Compile every output, check ownership and conflicts, then write. `--watch` regenerates on change and keeps the last good outputs on error |
| `check` | Fail when a generated file is missing, modified, stale or obsolete. Use it in CI |
| `validate` | Everything `generate` checks, without writing |
| `list` | Each generated file with its format, status and source |
| `explain` | Every generated key, its value and the source line it came from |
| `clean` | Remove generated files that are unchanged since ToolConfig wrote them |

`--strict` turns warnings, such as unused tool tables or values a tool table repeats, into
errors.

## Ownership

ToolConfig only writes files it owns. `.toolconfig-state/state.json` records each generated
path, relative to the root, with the hash of the bytes last written. Commit it: it carries no
machine-specific data, and a fresh clone needs it to recognize the committed files.

ToolConfig refuses to overwrite hand-written files, files edited after generation, symlinks,
and native files that compete with a generated one (`.prettierrc.yaml` beside a generated
`.prettierrc.json`, or a `prettier` key in `package.json`). It compiles and checks every output
before writing any, stages each change beside its destination, and replaces files atomically
under a journal that the next run recovers if a run is interrupted.

## Adopting ToolConfig

There is no importer. Converting a repository is a one-time job that an AI assistant handles
well; [docs/adopting.md](docs/adopting.md) is written for both people and assistants.

## Development

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --locked
python tests/native/run.py --toolconfig target/debug/toolconfig
```

The native suite runs rustfmt, ruff, prettier, oxfmt, tombi, yamllint and sqlfluff against
generated configuration; `mise install` provides them. After changing the catalog, refresh the
schema with `cargo run -- schema > schema/toolconfig.schema.json`. Releases are described in
[docs/releasing.md](docs/releasing.md).

## License

[MIT](LICENSE).
