"""Run real tools against generated configuration and check that they follow it.

Each case writes a source with distinctive EditorConfig values, generates native files with
ToolConfig, runs the tool, and checks the result for those values.
"""

import argparse
import json
import re
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


def golangci_lint(directory):
    (directory / "go.mod").write_text("module example.com/native\n\ngo 1.24\n")
    (directory / "main.go").write_text(
        'package main\n\nimport "fmt"\n\nfunc main() {\n\tfmt.Println("this line runs past forty columns")\n}\n'
    )
    result = run(["golangci-lint", "run", "./..."], directory, check=False)
    output = result.stdout + result.stderr
    assert "exceeds the maximum of 40 characters" in output, f"lll line-length = 40:\n{output}"


def tflint(directory):
    (directory / "main.tf").write_text(
        'variable "Region" {\n  type = string\n}\n\nvariable "zone_name" {\n  type = string\n}\n'
    )
    result = run(["tflint"], directory, check=False)
    output = result.stdout + result.stderr
    assert "`zone_name` must match the following format: Upper camel case" in output, (
        f"custom_formats and the variable block:\n{output}"
    )
    assert "`Region` must match" not in output, f"variable format = upper:\n{output}"


def shellcheck(directory):
    (directory / "lib dir").mkdir()
    (directory / "lib dir" / "common.sh").write_text("greeting=hello\n")
    (directory / "script").write_text('. common.sh\necho "$greeting"\nunused=1\n')
    result = run(["shellcheck", "script"], directory, check=False)
    codes = set(re.findall(r"SC\d+", result.stdout))
    assert codes == {"SC2250"}, f"shell, source-path, enable and disable:\n{result.stdout}"


def vale(directory):
    # Vale reads each list as items with commas between them. A comma inside an item is `\,`.
    # The patterns hide "xxx" and "yyy". Vale reports only "zzz".
    styles = directory / ".vale" / "styles"
    (styles / "Local").mkdir(parents=True)
    (styles / "Local" / "Letters.yml").write_text(
        "extends: existence\nmessage: Avoid '%s'.\nlevel: error\ntokens: [xxx, yyy, zzz]\n"
    )
    for name, word in (("Terms", "utilize"), ("Phrases", "leverage")):
        vocabulary = styles / "config" / "vocabularies" / name
        vocabulary.mkdir(parents=True)
        (vocabulary / "reject.txt").write_text(f"{word}\n")
    (directory / "notes.md").write_text("We utilize and leverage xxx yyy zzz.\n")
    result = run(["vale", "--no-global", "--output=JSON", "notes.md"], directory, check=False)
    assert result.returncode == 1, f"alerts:\n{result.stdout}{result.stderr}"
    alerts = [(alert["Check"], alert["Match"]) for alert in json.loads(result.stdout)["notes.md"]]
    assert alerts == [
        ("Vale.Avoid", "utilize"),
        ("Vale.Avoid", "leverage"),
        ("Local.Letters", "zzz"),
    ], f"Vocab, BasedOnStyles and TokenIgnores:\n{alerts}"


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
    "golangci-lint": (
        '["*.go"]\nmax_line_length = 40\n\n["*.go".golangci-lint.linters]\ndefault = "none"\nenable = ["lll"]\n',
        golangci_lint,
    ),
    "tflint": (
        (
            '["*.tf".tflint.rule.terraform_naming_convention]\nenabled = true\n'
            'custom_formats.upper = { description = "Upper camel case", regex = "^[A-Z][a-zA-Z]*$" }\n'
            'variable.format = "upper"\n'
        ),
        tflint,
    ),
    "shellcheck": (
        (
            '["*".shellcheck]\nshell = "bash"\nexternal-sources = true\n'
            'source-path = ["lib dir"]\nenable = ["require-variable-braces"]\ndisable = ["SC2034"]\n'
        ),
        shellcheck,
    ),
    "vale": (
        (
            '["*".vale]\nStylesPath = ".vale/styles"\nVocab = ["Terms", "Phrases"]\n\n'
            '["*".vale."*.md"]\nBasedOnStyles = ["Vale", "Local"]\n"Vale.Spelling" = "NO"\n'
            'TokenIgnores = ["(x{2,3})", "(y+)"]\n'
        ),
        vale,
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
