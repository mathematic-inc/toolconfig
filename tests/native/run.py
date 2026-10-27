"""Run real tools against generated configuration and check that they follow it.

Each case writes a source with distinctive EditorConfig values, generates native files with
ToolConfig, runs the tool, and checks the result for those values.
"""

import argparse
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path


def run(command, cwd, check=True):
    result = subprocess.run(command, cwd=cwd, capture_output=True, text=True, check=False)
    if check and result.returncode != 0:
        raise AssertionError(
            f"{' '.join(command)} exited {result.returncode}\n{result.stdout}{result.stderr}"
        )
    return result


def rustfmt(directory):
    (directory / "main.rs").write_text('fn main() {\nlet x = 1;\nprintln!("{x}");\n}\n')
    run(["rustfmt", "main.rs"], directory)
    assert "\n   let x = 1;" in (directory / "main.rs").read_text(), "tab_spaces = 3"


def ruff(directory):
    (directory / "module.py").write_text("def f():\n    return 1\n")
    run(["ruff", "format", "module.py"], directory)
    assert "\n  return 1" in (directory / "module.py").read_text(), "indent-width = 2"


def prettier(directory):
    # Prettier keeps an object expanded when it starts on its own line.
    (directory / "data.json").write_text('{\n"a": {\n"b": 1}}\n')
    (directory / "notes.md").write_text("word " * 20 + "\n")
    run(["prettier", "--write", "data.json", "notes.md"], directory)
    assert '\n        "b": 1' in (directory / "data.json").read_text(), "tabWidth = 4"
    lines = (directory / "notes.md").read_text().splitlines()
    assert max(len(line) for line in lines) <= 40 and len(lines) > 1, "md printWidth = 40"


def oxfmt(directory):
    (directory / "code.ts").write_text("function f(){return 1;}\n")
    run(["oxfmt", "code.ts"], directory)
    assert "\n    return 1\n" in (directory / "code.ts").read_text(), "tabWidth = 4, no semi"


def tombi(directory):
    (directory / "data.toml").write_text("values = [1111, 2222, 3333, 4444, 5555, 6666]\n")
    run(["tombi", "format", "data.toml"], directory)
    text = (directory / "data.toml").read_text()
    assert "\n   1111," in text, f"indent-width = 3 and line-width = 20:\n{text}"


def yamllint(directory):
    (directory / "data.yaml").write_text("a:\n  b: 1\n")
    result = run(["yamllint", "-f", "parsable", "data.yaml"], directory, check=False)
    assert "indentation" in result.stdout, f"indentation spaces = 4:\n{result.stdout}"


def sqlfluff(directory):
    (directory / "query.sql").write_text("SELECT first_column, second_column FROM some_table\n")
    result = run(["sqlfluff", "lint", "query.sql"], directory, check=False)
    assert "LT05" in result.stdout, f"max_line_length = 20:\n{result.stdout}"


CASES = {
    "rustfmt": ('["*.rs"]\nindent_size = 3\n', rustfmt),
    "ruff": ('["*.{py,pyi}"]\nindent_size = 2\n', ruff),
    "prettier": (
        (
            '["*"]\nindent_size = 4\n\n["*.md"]\nmax_line_length = 40\n\n'
            '["*.md".prettier]\nproseWrap = "always"\n'
        ),
        prettier,
    ),
    "oxfmt": ('["*"]\nindent_size = 4\noxfmt.semi = false\n', oxfmt),
    "tombi": ('["*.toml"]\nindent_size = 3\nmax_line_length = 20\n', tombi),
    "yamllint": ('["*.{yaml,yml}"]\nindent_size = 4\n', yamllint),
    "sqlfluff": (
        '["*.sql"]\nmax_line_length = 20\n\n["*.sql".sqlfluff.sqlfluff]\ndialect = "ansi"\n',
        sqlfluff,
    ),
}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--toolconfig", type=Path, required=True)
    parser.add_argument("tools", nargs="*", default=sorted(CASES))
    args = parser.parse_args()
    toolconfig = args.toolconfig.resolve()
    failures = []
    for tool in args.tools:
        sections, check = CASES[tool]
        if shutil.which(tool) is None:
            failures.append(f"{tool}: not installed")
            continue
        with tempfile.TemporaryDirectory(prefix=f"toolconfig-native-{tool}-") as temporary:
            directory = Path(temporary)
            (directory / ".toolconfig.toml").write_text(
                f'root = true\n\n[toolconfig]\ntools = ["{tool}"]\n\n{sections}'
            )
            try:
                run([str(toolconfig), "generate"], directory)
                check(directory)
                print(f"ok: {tool}")
            except AssertionError as error:
                failures.append(f"{tool}: {error}")
    for failure in failures:
        print(f"FAILED {failure}", file=sys.stderr)
    sys.exit(1 if failures else 0)


if __name__ == "__main__":
    main()
