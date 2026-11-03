//! End-to-end behavior of the `toolconfig` command.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Project {
    dir: tempfile::TempDir,
}

impl Project {
    fn new() -> Self {
        let dir = tempfile::Builder::new()
            .prefix("toolconfig-test-")
            .tempdir()
            .unwrap();
        Self { dir }
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.dir.path().canonicalize().unwrap().join(relative)
    }

    fn write(&self, relative: &str, text: &str) -> &Self {
        let path = self.path(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
        self
    }

    fn read(&self, relative: &str) -> String {
        std::fs::read_to_string(self.path(relative))
            .unwrap_or_else(|error| panic!("{relative}: {error}"))
    }

    fn exists(&self, relative: &str) -> bool {
        self.path(relative).exists()
    }

    fn run_in(&self, dir: &str, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_toolconfig"))
            .args(args)
            .current_dir(self.path(dir))
            .env_remove("TOOLCONFIG_ROOT")
            .env("TOOLCONFIG_PKL_CACHE_DIR", self.path(".pkl-cache"))
            .output()
            .unwrap()
    }

    fn run(&self, args: &[&str]) -> Output {
        self.run_in("", args)
    }

    fn ok(&self, args: &[&str]) -> String {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "{args:?} failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    fn fail(&self, args: &[&str]) -> String {
        let output = self.run(args);
        assert!(!output.status.success(), "{args:?} unexpectedly succeeded");
        String::from_utf8(output.stderr).unwrap()
    }
}

const EXAMPLE: &str = r#"root = true

[toolconfig]
tools = ["rustfmt", "oxfmt", "oxlint"]

["*"]
charset = "utf-8"
end_of_line = "lf"
insert_final_newline = true
trim_trailing_whitespace = true
indent_style = "space"
indent_size = 2
max_line_length = 100
oxfmt.semi = false

["*".oxlint.categories]
correctness = "deny"
suspicious = "deny"

["*.rs"]
indent_size = 4
rustfmt.edition = "2024"

["*.md"]
trim_trailing_whitespace = false
max_line_length = "off"

["*.sh"]
shell_variant = "bash"
switch_case_indent = true
"#;

const PYTHON: &str = r#"[toolconfig]
tools = ["ruff"]

["*.{py,pyi}"]
indent_size = 4

["*.{py,pyi}".ruff.lint]
select = ["E", "F", "I"]
per-file-ignores = { "tests/**" = ["S101"] }
"#;

fn example() -> Project {
    let project = Project::new();
    project
        .write(".toolconfig.toml", EXAMPLE)
        .write("python/.toolconfig.toml", PYTHON);
    project
}

#[test]
fn generates_editorconfig_and_native_files() {
    let project = example();
    project.ok(&["generate"]);
    insta::assert_snapshot!("editorconfig", project.read(".editorconfig"));
    insta::assert_snapshot!("python_editorconfig", project.read("python/.editorconfig"));
    insta::assert_snapshot!("rustfmt", project.read("rustfmt.toml"));
    insta::assert_snapshot!("ruff", project.read("python/ruff.toml"));
    insta::assert_snapshot!("oxfmt", project.read(".oxfmtrc.json"));
    insta::assert_snapshot!("oxlint", project.read(".oxlintrc.json"));
    insta::assert_snapshot!("state", project.read(".toolconfig-state/state.json"));
    assert!(
        project
            .read(".toolconfig-state/.gitignore")
            .contains("!state.json")
    );
    // Generation is idempotent and works from any directory inside the project.
    let output = project.run_in("python", &["generate"]);
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("(0 changed)"));
    assert!(
        project
            .ok(&["check"])
            .starts_with("Up to date: 6 generated files")
    );
}

#[test]
fn explain_and_list_show_provenance() {
    let project = example();
    project.ok(&["generate"]);
    insta::assert_snapshot!("explain_rustfmt", project.ok(&["explain", "rustfmt"]));
    insta::assert_snapshot!("list", project.ok(&["list"]));
}

#[test]
fn pkl_sources_match_toml_sources() {
    let toml = example();
    toml.ok(&["generate"]);
    let pkl = Project::new();
    pkl.write("python/.toolconfig.toml", PYTHON).write(
        ".toolconfig.pkl",
        &format!(
            r#"amends "package://github.com/mathematic-inc/toolconfig/releases/download/v{version}/toolconfig@{version}#/ToolConfig.pkl"

local width = 100

root = true

toolconfig {{
  tools = List("rustfmt", "oxfmt", "oxlint")
}}

sections {{
  ["*"] {{
    charset = "utf-8"
    end_of_line = "lf"
    insert_final_newline = true
    trim_trailing_whitespace = true
    indent_style = "space"
    indent_size = 2
    max_line_length = width
    oxfmt {{
      semi = false
    }}
    oxlint {{
      categories {{
        correctness = "deny"
        suspicious = "deny"
      }}
    }}
  }}
  ["*.rs"] {{
    indent_size = 4
    rustfmt {{
      edition = "2024"
    }}
  }}
  ["*.md"] {{
    trim_trailing_whitespace = false
    max_line_length = "off"
  }}
  ["*.sh"] {{
    properties {{
      ["shell_variant"] = "bash"
      ["switch_case_indent"] = true
    }}
  }}
}}
"#,
            version = env!("CARGO_PKG_VERSION")
        ),
    );
    pkl.ok(&["generate"]);
    for file in [
        "rustfmt.toml",
        ".oxfmtrc.json",
        ".oxlintrc.json",
        "python/ruff.toml",
        "python/.editorconfig",
    ] {
        assert_eq!(pkl.read(file), toml.read(file), "{file}");
    }
    // Pkl orders properties by declaration, so compare what editors resolve.
    for file in [
        "src/main.rs",
        "README.md",
        "run.sh",
        "python/app.py",
        "notes.txt",
    ] {
        assert_eq!(
            editorconfig(&pkl.path(file)),
            editorconfig(&toml.path(file)),
            "{file}"
        );
    }
}

fn editorconfig(path: &Path) -> Vec<(String, String)> {
    let mut properties = ec4rs::properties_of(path)
        .unwrap()
        .iter()
        .map(|(key, value)| (key.to_owned(), value.into_str().to_owned()))
        .collect::<Vec<_>>();
    properties.sort();
    properties
}

#[test]
fn check_reports_drift_and_generate_protects_edits() {
    let project = example();
    project.ok(&["generate"]);
    std::fs::write(
        project.path("rustfmt.toml"),
        project.read("rustfmt.toml") + "x = 1\n",
    )
    .unwrap();
    let stdout = String::from_utf8(project.run(&["check"]).stdout).unwrap();
    assert_eq!(stdout, "modified\trustfmt.toml\n");
    assert!(
        project
            .fail(&["generate"])
            .contains("was modified outside ToolConfig")
    );
    std::fs::remove_file(project.path("rustfmt.toml")).unwrap();
    project.write(".toolconfig.toml", &EXAMPLE.replace("\"2024\"", "\"2021\""));
    let stdout = String::from_utf8(project.run(&["check"]).stdout).unwrap();
    assert_eq!(stdout, "missing\trustfmt.toml\n");
    project.ok(&["generate"]);
    assert!(project.read("rustfmt.toml").contains("edition = \"2021\""));
    project.write(".toolconfig.toml", EXAMPLE);
    let stdout = String::from_utf8(project.run(&["check"]).stdout).unwrap();
    assert_eq!(stdout, "stale\trustfmt.toml\n");
}

#[test]
fn removing_a_tool_removes_its_unchanged_file() {
    let project = example();
    project.ok(&["generate"]);
    project.write(
        ".toolconfig.toml",
        &EXAMPLE.replace(
            r#"tools = ["rustfmt", "oxfmt", "oxlint"]"#,
            r#"tools = ["rustfmt"]"#,
        ),
    );
    let stderr = String::from_utf8(project.run(&["generate", "--strict"]).stderr).unwrap();
    assert!(
        stderr.contains("oxfmt options in section \"*\" are unused"),
        "{stderr}"
    );
    project.ok(&["generate"]);
    assert!(!project.exists(".oxfmtrc.json"));
    assert!(!project.exists(".oxlintrc.json"));
    assert!(project.exists("rustfmt.toml"));
}

#[test]
fn refuses_hand_written_files() {
    let project = example();
    project.write(".editorconfig", "root = true\n");
    let error = project.fail(&["generate"]);
    assert!(
        error.contains("refusing to overwrite unowned file .editorconfig"),
        "{error}"
    );
    std::fs::remove_file(project.path(".editorconfig")).unwrap();
    project.write("python/.ruff.toml", "line-length = 80\n");
    let error = project.fail(&["generate"]);
    assert!(
        error.contains("python/.ruff.toml would conflict"),
        "{error}"
    );
    std::fs::remove_file(project.path("python/.ruff.toml")).unwrap();
    project.write("docs/.editorconfig", "[*]\nindent_size = 3\n");
    let error = project.fail(&["generate"]);
    assert!(
        error.contains("docs/.editorconfig is hand-written"),
        "{error}"
    );
}

#[test]
fn nested_projects_are_separate() {
    let project = example();
    project.write(
        "vendor/lib/.editorconfig",
        "root = true\n[*]\nindent_size = 8\n",
    );
    project.write(
        "tools/.toolconfig.toml",
        "root = true\n[toolconfig]\ntools = [\"rustfmt\"]\n",
    );
    project.ok(&["generate"]);
    assert!(!project.exists("tools/rustfmt.toml"));
    let output = project.run_in("tools", &["generate"]);
    assert!(output.status.success());
    assert!(project.exists("tools/rustfmt.toml"));
    assert!(project.exists("tools/.toolconfig-state/state.json"));
}

#[test]
fn conflicting_values_are_errors() {
    let project = Project::new();
    project.write(
        ".toolconfig.toml",
        "root = true\n[toolconfig]\ntools = [\"rustfmt\"]\n[\"*\"]\nmax_line_length = 100\n[\"*.rs\"]\nrustfmt.max_width = 120\n",
    );
    let error = project.fail(&["validate"]);
    assert!(
        error.contains("rustfmt.max_width is set twice for *.rs files"),
        "{error}"
    );
    project.write(
        ".toolconfig.toml",
        "root = true\n[toolconfig]\ntools = [\"rustfmt\"]\nrustfmt.skip = [\"max_line_length\"]\n[\"*\"]\nmax_line_length = 100\n[\"*.rs\"]\nrustfmt.max_width = 120\n",
    );
    project.ok(&["generate"]);
    assert!(project.read("rustfmt.toml").contains("max_width = 120"));
}

#[test]
fn sections_for_some_files_must_agree() {
    let project = Project::new();
    project.write(
        ".toolconfig.toml",
        "root = true\n[toolconfig]\ntools = [\"rustfmt\"]\n[\"*\"]\nindent_size = 4\n[\"src/legacy/**.rs\"]\nindent_size = 2\n",
    );
    let error = project.fail(&["validate"]);
    assert!(
        error.contains("cannot follow section \"src/legacy/**.rs\""),
        "{error}"
    );
    assert!(
        error.contains("add a source in src/legacy that lists rustfmt"),
        "{error}"
    );
    project.write(
        "src/legacy/.toolconfig.toml",
        "[toolconfig]\ntools = [\"rustfmt\"]\n",
    );
    project.ok(&["generate"]);
    assert!(
        project
            .read("src/legacy/rustfmt.toml")
            .contains("tab_spaces = 2")
    );
    assert!(project.read("rustfmt.toml").contains("tab_spaces = 4"));
}

#[test]
fn overrides_carry_per_extension_values() {
    let project = Project::new();
    project.write(
        ".toolconfig.toml",
        "root = true\n[toolconfig]\ntools = [\"prettier\"]\n[\"*\"]\nindent_size = 2\nmax_line_length = 100\n[\"*.md\"]\nmax_line_length = 80\n",
    );
    project.ok(&["generate"]);
    insta::assert_snapshot!("prettier_overrides", project.read(".prettierrc.json"));
}

#[test]
fn custom_files_and_alternative_paths() {
    let project = Project::new();
    project.write(
        ".toolconfig.toml",
        r#"root = true

[toolconfig]
tools = ["typos"]
typos.path = "_typos.toml"

[toolconfig.files.prettierignore]
path = ".prettierignore"
format = "lines"
config = ["dist/", "coverage/"]

["*".typos.default.extend-words]
teh = "teh"
"#,
    );
    project.ok(&["generate"]);
    assert_eq!(project.read(".prettierignore"), "dist/\ncoverage/\n");
    assert_eq!(
        project.read("_typos.toml"),
        "[default.extend-words]\nteh = \"teh\"\n"
    );
}

#[test]
fn one_source_per_directory() {
    let project = example();
    project.write(".toolconfig", "root = true\n");
    assert!(
        project
            .fail(&["generate"])
            .contains("more than one ToolConfig source")
    );
}

#[test]
fn gitignore_block_skips_editorconfig() {
    let project = example();
    project.write(".gitignore", "target/\n");
    project.ok(&["init", "--gitignore"]);
    project.ok(&["generate"]);
    let ignore = project.read(".gitignore");
    assert!(ignore.contains("/rustfmt.toml\n"), "{ignore}");
    assert!(ignore.contains("/python/ruff.toml\n"), "{ignore}");
    assert!(!ignore.contains(".editorconfig"), "{ignore}");
}

#[test]
fn clean_removes_unchanged_outputs() {
    let project = example();
    project.ok(&["generate"]);
    project.ok(&["clean"]);
    for file in [".editorconfig", "rustfmt.toml", "python/ruff.toml"] {
        assert!(!project.exists(file), "{file}");
    }
    assert!(project.exists(".toolconfig.toml"));
}

#[test]
fn init_creates_sources() {
    let project = Project::new();
    assert!(project.ok(&["init"]).starts_with("Created"));
    let text = project.read(".toolconfig.toml");
    assert!(
        text.starts_with(
            "#:schema https://github.com/mathematic-inc/toolconfig/releases/download/v"
        )
    );
    project.ok(&["generate"]);
    assert!(
        project
            .read(".editorconfig")
            .contains("[*]\ncharset = utf-8\n")
    );
    assert!(project.ok(&["init"]).starts_with("Using"));

    // New projects ignore generated tool files by default.
    assert!(
        project
            .read(".gitignore")
            .contains("# BEGIN TOOLCONFIG GENERATED FILES")
    );
    let committed = Project::new();
    committed.ok(&["init", "--no-gitignore"]);
    assert!(!committed.exists(".gitignore"));

    let pkl = Project::new();
    pkl.ok(&["init", "--format", "pkl"]);
    pkl.ok(&["generate"]);
    assert_eq!(
        pkl.read(".editorconfig").lines().nth(1),
        Some("root = true")
    );

    let adopting = Project::new();
    adopting.write(".editorconfig", "root = true\n");
    assert!(adopting.fail(&["init"]).contains("is hand-written"));
}

#[test]
fn schema_is_current() {
    let project = Project::new();
    let schema = project.ok(&["schema"]);
    let committed = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("schema/toolconfig.schema.json"),
    )
    .unwrap();
    assert_eq!(
        schema, committed,
        "run `cargo run -- schema > schema/toolconfig.schema.json`"
    );
}

#[test]
fn examples_are_current() {
    let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples");
    for entry in std::fs::read_dir(&examples).unwrap() {
        let example = entry.unwrap().path();
        let output = Command::new(env!("CARGO_BIN_EXE_toolconfig"))
            .args(["check", "--strict"])
            .current_dir(&example)
            .env_remove("TOOLCONFIG_ROOT")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}: run `toolconfig generate` there\n{}{}",
            example.display(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn watch_ignores_package_cache_activity() {
    use std::io::{BufRead, BufReader};
    use std::process::Stdio;
    use std::sync::mpsc;
    use std::time::Duration;

    let project = Project::new();
    // A glob import watches the whole project directory, which contains the cache.
    project.write("fragments/width.pkl", "width = 100\n");
    project.write(
        ".toolconfig.pkl",
        &format!(
            "amends \"{}#/ToolConfig.pkl\"\n\nimport* \"fragments/*.pkl\" as fragments\n\nroot = true\n\ntoolconfig {{\n  tools = List(\"rustfmt\")\n}}\n\nsections {{\n  [\"*.rs\"] {{\n    max_line_length = fragments[\"fragments/width.pkl\"].width\n  }}\n}}\n",
            format_args!(
                "package://github.com/mathematic-inc/toolconfig/releases/download/v{0}/toolconfig@{0}",
                env!("CARGO_PKG_VERSION")
            )
        ),
    );
    let mut child = Command::new(env!("CARGO_BIN_EXE_toolconfig"))
        .args(["generate", "--watch"])
        .current_dir(project.path(""))
        .env_remove("TOOLCONFIG_ROOT")
        .env("TOOLCONFIG_PKL_CACHE_DIR", project.path(".pkl-cache"))
        .env("TOOLCONFIG_WATCH_TRACE", "1")
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let (sender, lines) = mpsc::channel();
    let stderr = child.stderr.take().unwrap();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if sender.send(line).is_err() {
                break;
            }
        }
    });
    let generated = |timeout| {
        lines
            .recv_timeout(timeout)
            .into_iter()
            .chain(std::iter::from_fn(|| lines.recv_timeout(timeout).ok()))
            .find(|line: &String| line.contains("generated") || line.contains("trace:"))
    };
    let first = generated(Duration::from_secs(60));
    assert!(first.is_some(), "watch mode never generated");
    let cache = std::fs::read_dir(project.path(".pkl-cache"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    std::fs::write(cache.join("unrelated"), "cache activity").unwrap();
    let again = generated(Duration::from_millis(1500));
    // A real edit to an imported module still regenerates.
    project.write("fragments/width.pkl", "width = 90\n");
    let edited = lines
        .recv_timeout(Duration::from_secs(60))
        .into_iter()
        .chain(std::iter::from_fn(|| {
            lines.recv_timeout(Duration::from_secs(60)).ok()
        }))
        .find(|line| line.contains("generated"));
    let _ = child.kill();
    let _ = child.wait();
    assert_eq!(again, None, "cache activity triggered regeneration");
    assert!(edited.is_some(), "an edited import did not regenerate");
    assert!(project.read("rustfmt.toml").contains("max_width = 90"));
}

#[test]
fn check_allows_missing_ignored_files() {
    let project = example();
    project.ok(&["init", "--gitignore"]);
    project.ok(&["generate"]);
    // A fresh clone lacks the ignored tool files, but must have .editorconfig.
    std::fs::remove_file(project.path("rustfmt.toml")).unwrap();
    std::fs::remove_file(project.path("python/ruff.toml")).unwrap();
    assert!(project.ok(&["check"]).starts_with("Up to date"));
    std::fs::remove_file(project.path(".editorconfig")).unwrap();
    let stdout = String::from_utf8(project.run(&["check"]).stdout).unwrap();
    assert_eq!(stdout, "missing\t.editorconfig\n");
    project.ok(&["generate"]);
    // An ignored file that exists is still checked.
    project.write(".toolconfig.toml", &EXAMPLE.replace("\"2024\"", "\"2021\""));
    let stdout = String::from_utf8(project.run(&["check"]).stdout).unwrap();
    assert_eq!(stdout, "stale\trustfmt.toml\n");
}

/// Checks that `check` reports `file` once it is edited, and once `changed` replaces the source.
fn assert_check_catches_drift(project: &Project, file: &str, changed: &str) {
    let generated = project.read(file);
    project.write(file, &format!("{generated}# edited\n"));
    let stdout = String::from_utf8(project.run(&["check"]).stdout).unwrap();
    assert_eq!(stdout, format!("modified\t{file}\n"));
    project.write(file, &generated);
    let source = project.read(".toolconfig.toml");
    project.write(".toolconfig.toml", changed);
    let stdout = String::from_utf8(project.run(&["check"]).stdout).unwrap();
    assert_eq!(stdout, format!("stale\t{file}\n"));
    project.write(".toolconfig.toml", &source);
    assert!(project.ok(&["check"]).starts_with("Up to date"));
}

const GOLANGCI_LINT: &str = r#"root = true

[toolconfig]
tools = ["golangci-lint"]

["*"]
max_line_length = 100

["*.go".golangci-lint.linters]
enable = ["lll", "misspell"]

["*.go".golangci-lint.formatters]
enable = ["gofumpt", "goimports"]
settings.goimports.local-prefixes = ["example.com/acme"]
"#;

#[test]
fn golangci_lint_takes_the_width_where_lll_is_on() {
    let project = Project::new();
    project.write(".toolconfig.toml", GOLANGCI_LINT);
    project.ok(&["generate"]);
    insta::assert_snapshot!("golangci_lint", project.read(".golangci.yml"));
    insta::assert_snapshot!(
        "explain_golangci_lint",
        project.ok(&["explain", "golangci-lint"])
    );
    assert_check_catches_drift(
        &project,
        ".golangci.yml",
        &GOLANGCI_LINT.replace("misspell", "revive"),
    );
    // lll counts as on in the default sets that include it, and off once disabled.
    for (linters, on) in [
        ("enable = [\"misspell\"]", false),
        ("default = \"fast\"", true),
        ("default = \"all\"\ndisable = [\"lll\"]", false),
    ] {
        project.write(
            ".toolconfig.toml",
            &GOLANGCI_LINT.replace("enable = [\"lll\", \"misspell\"]", linters),
        );
        project.ok(&["generate"]);
        let generated = project.read(".golangci.yml");
        assert_eq!(generated.contains("line-length: 100"), on, "{generated}");
    }
    project.write(
        ".toolconfig.toml",
        &GOLANGCI_LINT.replace(
            "\"misspell\"]",
            "\"misspell\"]\nsettings.lll.line-length = 80",
        ),
    );
    let error = project.fail(&["validate"]);
    assert!(
        error
            .contains("golangci-lint.linters.settings.lll.line-length is set twice for *.go files"),
        "{error}"
    );
}

const HADOLINT: &str = r#"root = true

[toolconfig]
tools = ["hadolint"]

["*".hadolint]
ignored = ["DL3008"]
trustedRegistries = ["docker.io", "*.gcr.io"]
failure-threshold = "warning"
strict-labels = true

["*".hadolint.override]
error = ["DL3001"]
"#;

#[test]
fn hadolint_receives_native_options() {
    let project = Project::new();
    project.write(".toolconfig.toml", HADOLINT);
    project.ok(&["generate"]);
    insta::assert_snapshot!("hadolint", project.read(".hadolint.yaml"));
    insta::assert_snapshot!("explain_hadolint", project.ok(&["explain", "hadolint"]));
    assert_check_catches_drift(
        &project,
        ".hadolint.yaml",
        &HADOLINT.replace("DL3008", "DL3009"),
    );
    project.write(".hadolint.yml", "ignored: []\n");
    let error = project.fail(&["generate"]);
    assert!(error.contains(".hadolint.yml would conflict"), "{error}");
}

const TFLINT: &str = r#"root = true

[toolconfig]
tools = ["tflint"]

["*.tf".tflint.config]
call_module_type = "local"
format = "compact"
ignore_module = { "terraform-aws-modules/vpc/aws" = true }

["*.tf".tflint.plugin.aws]
enabled = true
version = "0.50.0"
source = "github.com/terraform-linters/tflint-ruleset-aws"

["*.tf".tflint.rule.terraform_naming_convention]
enabled = true
variable.format = "snake_case"

["*.tf".tflint.rule.aws_instance_invalid_type]
enabled = false
"#;

#[test]
fn tflint_is_written_in_hcl() {
    let project = Project::new();
    project.write(".toolconfig.toml", TFLINT);
    project.ok(&["generate"]);
    insta::assert_snapshot!("tflint", project.read(".tflint.hcl"));
    insta::assert_snapshot!("explain_tflint", project.ok(&["explain", "tflint"]));
    assert_check_catches_drift(
        &project,
        ".tflint.hcl",
        &TFLINT.replace("enabled = false", "enabled = true"),
    );
    // An object the schema does not place is an error; JSON leaves it to tflint.
    let assume_role = format!(
        "{TFLINT}\n[\"*.tf\".tflint.plugin.aws.assume_role]\nrole_arn = \"arn:aws:iam::123456789012:role/lint\"\n"
    );
    project.write(".toolconfig.toml", &assume_role);
    let error = project.fail(&["validate"]);
    assert!(
        error.contains("cannot tell whether plugin.aws.assume_role is a block or an attribute"),
        "{error}"
    );
    assert!(error.contains("format = \"json\""), "{error}");
    project.write(
        ".toolconfig.toml",
        &assume_role.replace(
            "tools = [\"tflint\"]",
            "tools = [\"tflint\"]\ntflint.format = \"json\"",
        ),
    );
    project.ok(&["generate"]);
    assert!(!project.exists(".tflint.hcl"));
    assert!(project.read(".tflint.json").contains("\"assume_role\": {"));
}

const SHELLCHECK: &str = r#"root = true

[toolconfig]
tools = ["shellcheck"]

["*".shellcheck]
shell = "bash"
external-sources = true
enable = ["quote-safe-variables", "require-variable-braces"]
disable = "SC2236"
source-path = ["SCRIPTDIR", "lib dir"]
"#;

#[test]
fn shellcheck_writes_its_rc_file() {
    let project = Project::new();
    project.write(".toolconfig.toml", SHELLCHECK);
    project.ok(&["generate"]);
    insta::assert_snapshot!("shellcheck", project.read(".shellcheckrc"));
    insta::assert_snapshot!("explain_shellcheck", project.ok(&["explain", "shellcheck"]));
    assert_check_catches_drift(
        &project,
        ".shellcheckrc",
        &SHELLCHECK.replace("SC2236", "SC2034"),
    );
    // The rc file covers every script beneath it, so options for some scripts cannot go there.
    project.write(
        ".toolconfig.toml",
        &format!("{SHELLCHECK}\n[\"*.bash\".shellcheck]\nshell = \"bash\"\ndisable = \"SC2034\"\n"),
    );
    let error = project.fail(&["validate"]);
    assert!(
        error.contains("cannot follow section \"*.bash\""),
        "{error}"
    );
}
