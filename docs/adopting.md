# Adopting ToolConfig

This guide converts a repository's hand-written `.editorconfig` and tool configuration files
into one ToolConfig source. It is written so that an AI assistant can follow it exactly; you
can also follow it by hand. Ask your assistant:

> Convert this repository to ToolConfig by following
> <https://github.com/mathematic-inc/toolconfig/blob/main/docs/adopting.md>, and verify the
> result as the guide describes.

## 1. Take stock

1. Copy the current `.editorconfig` files and every native configuration file aside, outside
   the repository, so you can compare later.
2. List which tools have native files. The supported tools are actionlint, buf, cargo-deny,
   csharpier, editorconfig-checker, gitleaks, knip, lychee, oxfmt, oxlint, prettier, ruff,
   rumdl, rustfmt, ryl, sqlfluff, tombi, ty, typespec, typos, yamlfmt and yamllint. Leave
   files for other tools as they are; ToolConfig does not touch them.

## 2. Convert `.editorconfig`

Create `.editorconfig.toml` (or `.toolconfig.toml`) beside each `.editorconfig`:

- Move `root = true` to the top.
- Turn each `[glob]` header into a quoted key: `[*.{py,sql}]` becomes `["*.{py,sql}"]`.
- Quote string values. Keep integers and `true`/`false` bare. `unset`, `off` and `tab` are
  strings: `indent_size = "unset"`.
- Keep dotted keys such as `dotnet_diagnostic.CA1822.severity = "warning"`; they are written
  back to `.editorconfig` unchanged.
- Keep comments; they stay in the source.
- Keep the section order. If a glob appears twice, merge the second into the first, unless a
  section between them sets the same property.

## 3. Move tool options into the source

For each native file:

1. Add the tool's id to `tools` in `[toolconfig]` in the source of that directory.
2. If the file uses a non-default name or format, set `<tool>.path` or `<tool>.format`, such as
   `typos.path = "_typos.toml"`.
3. Move the options into a table under the `["*"]` section, keeping their structure:
   `["*".ruff]`, `["*".ruff.lint]`, `[["*".gitleaks.allowlists]]`. Options that only apply to
   some file types can go under that type's section, such as `["*.md".prettier]`.
4. Formatting options become standard properties instead, in a section for the files the tool
   formats. Each pair below is one value:

   | Tool option | Standard property |
   | --- | --- |
   | prettier/oxfmt/csharpier `printWidth`, rustfmt `max_width`, ruff `line-length`, sqlfluff `max_line_length`, tombi `line-width` | `max_line_length` |
   | prettier/oxfmt/csharpier `tabWidth`, rustfmt `tab_spaces`, ruff `indent-width`, sqlfluff `tab_space_size` | `indent_size` |
   | prettier/oxfmt/csharpier `useTabs`, rustfmt `hard_tabs`, ruff `format.indent-style` | `indent_style` |
   | prettier/oxfmt/csharpier `endOfLine`, rustfmt `newline_style`, ruff `format.line-ending` | `end_of_line` |

   If the tool's value differs from what `.editorconfig` says for the same files, decide which
   one is right and keep only that. To keep a deliberate difference, leave the option in the
   tool table and add the property to `[toolconfig.<tool>] skip`.
5. Delete the native file.

Adding `max_line_length` makes editors show a ruler, and makes editorconfig-checker enforce it
unless its `Disable.MaxLineLength` option is set.

## 4. Generate and verify

1. Delete each converted `.editorconfig`. ToolConfig refuses to overwrite hand-written files.
2. Run `toolconfig generate`. If it reports that a section cannot be followed, or that a value
   is set twice, fix the source as the message suggests.
3. Run `toolconfig explain` and compare each native file with the copy from step 1. Differences
   should be limited to:
   - key order and formatting;
   - values a tool already used by default, now written explicitly;
   - values that now follow `.editorconfig`, which `explain` attributes to standard properties.
4. Compare `.editorconfig` with the original. Only comments, formatting and the header should
   differ, unless you aligned values in step 3.
5. Run the repository's formatters and linters. They should report what they reported before.
6. Run `toolconfig init --gitignore` to add the managed `.gitignore` block, then
   `git rm --cached` the generated tool files it lists; they stay on disk. Commit the source,
   `.editorconfig`, `.gitignore` and `.toolconfig-state/state.json`.
7. Make sure the tool files exist wherever tools run: add `toolconfig generate` to a pre-commit
   hook or task runner and to CI before the formatters and linters, and `toolconfig check` to CI.
