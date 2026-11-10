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
changes. Generated tool files are gitignored by default, so run `toolconfig generate` after
cloning, in a pre-commit hook or task runner, and in CI before the tools run. Commit the source,
`.editorconfig` and `.toolconfig-state/state.json`: editors and other programs read `.editorconfig`
directly, and the state file lets a fresh clone keep owning it.

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
  | `extends` | Pinned Git repositories to inherit, in declaration order |
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

Add `#:schema https://github.com/mathematic-inc/toolconfig/releases/download/v2026.11.2/toolconfig.schema.json`
at the top for completion in Taplo, Tombi and other TOML editors.

### Pkl sources

```pkl
amends "package://github.com/mathematic-inc/toolconfig/releases/download/v2026.11.2/toolconfig@2026.11.2#/ToolConfig.pkl"

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

### Shared configuration from Git

Import a complete configuration from a public or private repository:

```toml
root = true

[toolconfig]
extends = [
  { git = "https://github.com/acme/toolconfig.git", rev = "0123456789abcdef0123456789abcdef01234567" }
]

["*.rs"]
max_line_length = 100
```

Replace the example URL and commit ID with your repository and a full commit ID. Git must be
installed for the first fetch. Branch names, tags and abbreviated IDs are rejected. To update
shared settings, change `rev` and run `toolconfig generate`; restore the previous ID to roll back.

Omit `path` to use the repository root's single ToolConfig source, or select a source with
`path = "presets/rust/.toolconfig.toml"`. All four source filenames are supported. ToolConfig
loads that source and its imports; it does not discover other configurations in the repository.
Symlinks and submodules are not materialized, and repository hooks and checkout filters do not run.

Imports apply depth first, in declaration order, followed by the consuming source. Imported
sections retain their order, even when globs repeat. Native tables merge key by key; arrays
replace earlier arrays. Standard properties retain EditorConfig's `unset` behavior.

An omitted `tools` or tool `skip` list inherits the earlier list. An explicit list replaces it;
`tools = []` disables inherited tools and `skip = []` clears inherited skips. Tool `format` and
`path` settings override individual fields. Custom files inherit by label; a later definition
replaces that label's complete `path`, `format` and `config` specification.

Globs and generated paths apply relative to the consuming source's directory. Only its `root`
setting controls the project boundary. Pkl relative imports still resolve beside the fetched
Pkl source; `TOOLCONFIG_SOURCE_DIR` points there and `TOOLCONFIG_ROOT_DIR` identifies the consumer's
project. Imported Pkl has the same filesystem, environment and network access as local Pkl: use
repositories you trust. `toolconfig explain` identifies the repository, commit and source of
inherited options.

In Pkl, use the schema URI written by `toolconfig init --format pkl` and quote the `extends`
keyword with backticks:

```pkl
toolconfig {
  `extends` = List(new Repository {
    git = "git@github.com:acme/toolconfig.git"
    rev = "0123456789abcdef0123456789abcdef01234567"
    path = ".toolconfig.pkl"
  })
  rustfmt { skip = List() }
}
```

The new Pkl schema uses `null` for omitted `tools` and `skip` lists. Explicit `List()` clears an
inherited list. Existing pinned schemas retain their empty-list defaults; update the schema URI
to use repository imports.

Authentication comes from Git. Public HTTPS needs no login. Private HTTPS uses your Git
credential helper; SSH URLs use your SSH configuration and agent, including `GIT_SSH_COMMAND`.
In CI, configure a helper or SSH key with read access to the shared repository. Test access with
`git ls-remote <url>` without prompting. ToolConfig disables Git terminal prompts and times out
Git commands after two minutes. Keep tokens out of URLs and sources; ToolConfig rejects embedded
HTTPS credentials and suppresses Git's raw error output to avoid disclosing credentials.

Fetched commits are stored in the user's cache directory under `toolconfig/repos`. Set
`TOOLCONFIG_REPO_CACHE_DIR` to choose another location; relative paths resolve from the project
root. Once fetched, a commit loads without Git or reauthentication, including while offline.
Pkl's own external dependencies still follow its existing cache and offline settings. Watch mode
does not poll remotes. ToolConfig verifies cached contents and ignores cache activity when
discovering sources or watching for changes. A damaged cache produces an error with the entry
to remove before retrying. Fetch or parse failures leave existing generated files intact.

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
| golangci-lint | width, as `lll`'s line length where the source turns `lll` on | `.golangci.yml` |
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
| actionlint, buf, cargo-deny, editorconfig-checker, gitleaks, hadolint, knip, lychee, oxlint, shellcheck, tflint, ty, typespec, vale | native options only | their usual file |

shfmt and ktlint read `.editorconfig` themselves; write their keys in sections.

golangci-lint's `lll` is on when `linters.enable` names it, or `linters.default` is `all` or
`fast`, and `linters.disable` does not name it. shellcheck's `.shellcheckrc` applies to every
script beneath it, so its options go in `["*"]`; an array repeats its key, as in
`source-path = ["SCRIPTDIR", "lib"]`. tflint's `.tflint.hcl` is written as HCL reads its JSON
form: `["*.tf".tflint.plugin.aws]` becomes `plugin "aws" { ... }`. ToolConfig knows which objects
are blocks in tflint's own schema and its bundled terraform ruleset; for a plugin's nested block,
such as `assume_role`, set `tflint.format = "json"` to write `.tflint.json`, which each plugin
reads by its own schema.

## Vale

Vale reads `.vale.ini` and a `StylesPath` folder of styles and vocabularies. ToolConfig writes the
INI file from the `vale` table in `["*"]`:

```toml
[toolconfig]
tools = ["vale"]

["*".vale]
StylesPath = ".vale/styles"
MinAlertLevel = "warning"
Vocab = "Acme"

["*".vale.formats]
rs = "md"

["*".vale."*.{md,rs}"]
BasedOnStyles = ["Vale", "Acme"]
"Vale.Spelling" = "NO"
```

- Core settings go directly in the table.
- A nested table is a section of the file, named by its glob. `["*".vale."*.md"]` writes `[*.md]`,
  and `["*".vale.formats]` writes `[formats]`. Vale reads one INI file per run, so every option
  goes in `["*"]`, as for shellcheck.
- A list is one comma-separated entry. ToolConfig writes a comma inside an item as `\,`, which is
  how Vale reads a comma in a pattern.
- Quote a key that has a dot, as in `"Vale.Spelling" = "NO"`. TOML reads an unquoted dot as a
  nested table.

Styles and vocabularies are plain files, and a `files` output can write them. A shared repository
can write them into every project that extends it. This block of its `.toolconfig.pkl` writes the
rule files of the `styles` folder and one vocabulary:

```pkl
toolconfig {
  files {
    for (uri, rule in read*("styles/*.yml")) {
      [uri] {
        path = ".vale/styles/Acme/\(uri.split("/").last)"
        format = "text"
        config = rule.text
      }
    }
    ["vale-acme"] {
      path = ".vale/styles/config/vocabularies/Acme/reject.txt"
      format = "lines"
      config = List("(?i)utilize")
    }
  }
}
```

`format = "text"` writes a string as it is. `format = "lines"` writes a list of strings, one on
each line, which is how Vale reads `accept.txt` and `reject.txt`. As for `.vale.ini`, `check`
reports an edit to these files, and `init --gitignore` lists them in the managed `.gitignore` block.

## Commands

```text
toolconfig [--root DIR] init [--format toml|pkl] [--gitignore | --no-gitignore]
toolconfig [--root DIR] generate [--watch] [--strict]
toolconfig [--root DIR] check [--strict]
toolconfig [--root DIR] validate [--strict]
toolconfig [--root DIR] list
toolconfig [--root DIR] explain [TOOL] [--dir DIR]
toolconfig [--root DIR] clean
```

| Command | Effect |
| --- | --- |
| `init` | Create a root source, preserving an existing one. A new source gets a managed `.gitignore` block listing the generated files other than `.editorconfig`; `--no-gitignore` commits them instead, and `--gitignore` adds the block to an existing project |
| `generate` | Compile every output, check ownership and conflicts, then write. `--watch` regenerates on change and keeps the last good outputs on error |
| `check` | Fail when a generated file is modified, stale or obsolete, or missing when it is committed. Ignored files may be missing, as in a fresh clone. Use it in CI |
| `validate` | Everything `generate` checks, without writing |
| `list` | Each generated file with its format, status and source |
| `explain` | Every generated key, its value and the source line it came from |
| `clean` | Remove generated files that are unchanged since ToolConfig wrote them |

`--strict` turns warnings, such as unused tool tables or values a tool table repeats, into
errors.

## Ownership

ToolConfig only writes files it owns. `.toolconfig-state/state.json` records each generated
path, relative to the root, with the hash of the bytes last written. Commit it: it carries no
machine-specific data, and a fresh clone needs it to recognize the committed `.editorconfig`.
Ignored files missing from a clone are simply generated again.

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

The native suite runs rustfmt, ruff, prettier, oxfmt, tombi, yamllint, sqlfluff, golangci-lint,
tflint, shellcheck and Vale against generated configuration; `mise install` provides them. After
changing the catalog, refresh the schema with `cargo run -- schema > schema/toolconfig.schema.json`.
Releases are described in [docs/releasing.md](docs/releasing.md).

## License

[MIT](LICENSE).
