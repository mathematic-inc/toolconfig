//! Repository imports through the CLI and real Git transports.

#![warn(rust_2018_idioms)]

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

mod support {
    pub(crate) mod git_http;
}

struct Fixture {
    directory: tempfile::TempDir,
}

const URL: &str = "https://toolconfig.test/settings.git";

impl Fixture {
    fn new() -> Self {
        let fixture = Self {
            directory: tempfile::tempdir().unwrap(),
        };
        fs::create_dir_all(fixture.path("project")).unwrap();
        fs::create_dir_all(fixture.path("source")).unwrap();
        fs::create_dir_all(fixture.path("empty-hooks")).unwrap();
        fixture.write("gitconfig", "");
        fixture.git(&["init", "--quiet", "--initial-branch=main"]);
        fixture.git_config("user.name", "ToolConfig fixture");
        fixture.git_config("user.email", "fixture@example.test");
        fixture.git_config("commit.gpgsign", "false");
        fixture.git_config(
            "core.hooksPath",
            fixture.path("empty-hooks").to_str().unwrap(),
        );
        fixture.transport(&format!(
            "file:///{}",
            fixture
                .path("source")
                .to_string_lossy()
                .replace('\\', "/")
                .trim_start_matches('/')
        ));
        fixture
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.directory.path().join(relative)
    }

    fn write(&self, relative: &str, text: &str) {
        let path = self.path(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn git(&self, args: &[&str]) -> String {
        let output = self.git_command().args(args).output().unwrap();
        assert!(
            output.status.success(),
            "Git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }

    fn git_command(&self) -> Command {
        let mut command = Command::new("git");
        command.current_dir(self.path("source"));
        self.isolate(&mut command);
        command
    }

    fn isolate(&self, command: &mut Command) {
        command
            .env("GIT_CONFIG_GLOBAL", self.path("gitconfig"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_COUNT", "0")
            .env_remove("GIT_CONFIG_PARAMETERS")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_SSH_COMMAND")
            .env_remove("GIT_SSH")
            .env_remove("GIT_ASKPASS")
            .env_remove("SSH_ASKPASS");
    }

    fn git_config(&self, key: &str, value: &str) {
        self.git(&[
            "config",
            "--file",
            self.path("gitconfig").to_str().unwrap(),
            key,
            value,
        ]);
    }

    fn transport(&self, target: &str) {
        self.git_config(&format!("url.{target}.insteadOf"), URL);
    }

    fn commit(&self) -> String {
        self.git(&["add", "--all"]);
        self.git(&["commit", "--quiet", "--allow-empty", "-m", "fixture"]);
        self.git(&["rev-parse", "HEAD"])
    }

    fn configure(&self, revision: &str, extra: &str) {
        self.write("project/.toolconfig.toml", &format!("root = true\n[toolconfig]\nextends = [{{ git = {URL:?}, rev = {revision:?} }}]\n{extra}"));
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_toolconfig"));
        command
            .args(args)
            .current_dir(self.path("project"))
            .env_remove("TOOLCONFIG_ROOT")
            .env("TOOLCONFIG_REPO_CACHE_DIR", self.path("project/.repos"))
            .env("TOOLCONFIG_PKL_CACHE_DIR", self.path("project/.pkl-cache"));
        self.isolate(&mut command);
        command
    }

    fn ok(&self, args: &[&str]) -> String {
        let output = self.command(args).output().unwrap();
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    fn fail(&self, args: &[&str]) -> String {
        let output = self.command(args).output().unwrap();
        assert!(!output.status.success(), "{args:?} unexpectedly succeeded");
        String::from_utf8(output.stderr).unwrap()
    }

    fn read(&self, relative: &str) -> String {
        fs::read_to_string(self.path(&format!("project/{relative}"))).unwrap()
    }
}

const SHARED: &str = r#"root = true
[toolconfig]
tools = ["rustfmt"]
rustfmt.skip = ["max_line_length"]
[toolconfig.files.notice]
path = "NOTICE.txt"
format = "text"
config = "shared"
["*"]
indent_style = "space"
indent_size = 4
max_line_length = 90
["*.rs"]
rustfmt.edition = "2024"
"#;

#[test]
fn full_config_inherits_and_local_values_override() {
    let fixture = Fixture::new();
    fixture.write("source/.toolconfig.toml", SHARED);
    let revision = fixture.commit();
    fixture.configure(
        &revision,
        r#"rustfmt.skip = []
[toolconfig.files.notice]
path = "NOTICE.txt"
format = "text"
config = "local"
["*.rs"]
max_line_length = 100
"#,
    );
    fixture.ok(&["generate"]);
    let native = fixture.read("rustfmt.toml");
    assert!(native.contains("max_width = 100"), "{native}");
    assert!(native.contains("tab_spaces = 4"), "{native}");
    assert_eq!(fixture.read("NOTICE.txt"), "local");
    let properties = ec4rs::properties_of(fixture.path("project/example.rs")).unwrap();
    assert_eq!(
        properties
            .iter()
            .find(|(key, _)| *key == "max_line_length")
            .unwrap()
            .1
            .into_str(),
        "100"
    );
    let explanation = fixture.ok(&["explain", "rustfmt"]);
    assert!(
        explanation.contains(&format!("{URL}@{revision}:.toolconfig.toml")),
        "{explanation}"
    );
    assert!(!explanation.contains(".repos"), "{explanation}");
    fixture.ok(&["check"]);
    fixture.ok(&["validate"]);
    assert!(fixture.ok(&["list"]).contains("rustfmt.toml"));
}

#[test]
fn cached_commit_does_not_fetch_again_and_clean_needs_no_remote() {
    let fixture = Fixture::new();
    fixture.write("source/.toolconfig.toml", SHARED);
    let revision = fixture.commit();
    fixture.configure(&revision, "");
    fixture.ok(&["generate"]);
    let state = fixture.read(".toolconfig-state/state.json");
    fs::remove_dir_all(fixture.path("source")).unwrap();
    assert_success(
        fixture
            .command(&["generate"])
            .env("PATH", fixture.path("no-executables"))
            .output()
            .unwrap(),
    );
    assert_eq!(fixture.read(".toolconfig-state/state.json"), state);
    fs::remove_dir_all(fixture.path("project/.repos")).unwrap();
    fixture.ok(&["clean"]);
    assert!(!fixture.path("project/rustfmt.toml").exists());
    assert!(!fixture.path("project/.repos").exists());
}

#[test]
fn explicit_empty_tools_disables_inherited_generation_settings() {
    let fixture = Fixture::new();
    fixture.write("source/.toolconfig.toml", SHARED);
    let revision = fixture.commit();
    fixture.configure(&revision, "tools = []\n");
    fixture.ok(&["generate"]);
    assert!(!fixture.path("project/rustfmt.toml").exists());
    assert!(fixture.path("project/NOTICE.txt").exists());
    fixture.configure(&revision, "tools = []\nrustfmt.skip = []\n");
    assert!(fixture.fail(&["validate"]).contains("missing from tools"));
}

#[test]
fn ordered_imports_preserve_omitted_lists_and_overlapping_globs() {
    let fixture = Fixture::new();
    fixture.write("source/base/.toolconfig.toml", SHARED);
    fixture.write(
        "source/other/.toolconfig.toml",
        "[toolconfig.rustfmt]\nskip = []\n[\"*.rs\"]\nindent_size = 8\n[\"*\"]\nmax_line_length = 110\n",
    );
    let revision = fixture.commit();
    fixture.write(
        "project/.toolconfig.toml",
        &format!(
            r#"root = true
[toolconfig]
extends = [
  {{ git = {URL:?}, rev = {revision:?}, path = "base/.toolconfig.toml" }},
  {{ git = {URL:?}, rev = {revision:?}, path = "other/.toolconfig.toml" }}
]
["*"]
indent_size = 2
"#
        ),
    );
    fixture.ok(&["generate"]);
    let native = fixture.read("rustfmt.toml");
    assert!(native.contains("tab_spaces = 2"), "{native}");
    assert!(native.contains("max_width = 110"), "{native}");
    assert_eq!(fixture.read(".editorconfig").matches("[*]").count(), 3);
    fixture.write(
        "project/.toolconfig.toml",
        "root = true\n[toolconfig.rustfmt]\nskip = []\n",
    );
    assert!(fixture.fail(&["validate"]).contains("missing from tools"));
}

#[test]
fn recursive_imports_and_nested_consumers_keep_local_scope() {
    let fixture = Fixture::new();
    fixture.write("source/.toolconfig.toml", SHARED);
    let base = fixture.commit();
    fixture.write("source/.toolconfig.toml", &format!("root = true\n[toolconfig]\nextends = [{{ git = {URL:?}, rev = {base:?} }}]\n[\"*.rs\"]\nindent_size = 6\n"));
    let revision = fixture.commit();
    fixture.write("project/.toolconfig.toml", "root = true\n");
    fixture.write(
        "project/crate/.toolconfig.toml",
        &format!("[toolconfig]\nextends = [{{ git = {URL:?}, rev = {revision:?} }}]\n"),
    );
    fixture.ok(&["generate"]);
    assert!(!fixture.path("project/rustfmt.toml").exists());
    assert!(
        fixture
            .read("crate/rustfmt.toml")
            .contains("tab_spaces = 6")
    );
    assert_eq!(fixture.read("crate/NOTICE.txt"), "shared");
    assert!(!fixture.read("crate/.editorconfig").contains("root = true"));
}

#[test]
fn unavailable_and_invalid_imports_preserve_generated_files() {
    let fixture = Fixture::new();
    fixture.write("source/.toolconfig.toml", SHARED);
    let revision = fixture.commit();
    fixture.configure(&revision, "");
    fixture.ok(&["generate"]);
    let before = fixture.read("rustfmt.toml");
    let state = fixture.read(".toolconfig-state/state.json");
    fixture.configure(&"f".repeat(40), "");
    assert!(fixture.fail(&["generate"]).contains("Git failed"));
    assert_eq!(fixture.read("rustfmt.toml"), before);
    assert_eq!(fixture.read(".toolconfig-state/state.json"), state);
    fixture.write("source/.toolconfig.toml", "[\"*\"]\nindent_size = 0\n");
    let invalid = fixture.commit();
    fixture.configure(&invalid, "");
    let error = fixture.fail(&["generate"]);
    assert!(error.contains(&format!("{URL}@{invalid}")), "{error}");
    assert!(!error.contains(".repos"), "{error}");
    assert_eq!(fixture.read("rustfmt.toml"), before);
    assert_eq!(fixture.read(".toolconfig-state/state.json"), state);
}

#[test]
fn invalid_specs_fail_without_fetching_or_disclosing_credentials() {
    let fixture = Fixture::new();
    for spec in [
        "{ git = 'https://reader:fixture-secret@example.test/repo', rev = 'main' }",
        "{ git = 'https://example.test/repo?token=fixture-secret', rev = 'main' }",
        "{ git = 'https://example.test/repo', rev = 'main' }",
        "{ git = 'ext::fixture-secret', rev = '1111111111111111111111111111111111111111' }",
        "{ git = 'https://example.test/repo', rev = '1111111111111111111111111111111111111111', path = '../.toolconfig.toml' }",
        "{ git = 'https://example.test/repo', rev = '1111111111111111111111111111111111111111', path = '/.toolconfig.toml' }",
    ] {
        fixture.write(
            "project/.toolconfig.toml",
            &format!("root = true\n[toolconfig]\nextends = [{spec}]\n"),
        );
        let error = fixture.fail(&["generate"]);
        assert!(!error.contains("fixture-secret"), "{error}");
        assert!(!fixture.path("project/.repos").exists());
    }
}

#[test]
fn pkl_imports_resolve_relative_modules_and_explicit_empty_lists() {
    let fixture = Fixture::new();
    let package = format!(
        "package://github.com/mathematic-inc/toolconfig/releases/download/v{0}/toolconfig@{0}#/ToolConfig.pkl",
        env!("CARGO_PKG_VERSION")
    );
    fixture.write("source/width.pkl", "width = 96\n");
    fixture.write(
        "source/.toolconfig.pkl",
        &format!(
            r#"amends "{package}"
import "width.pkl" as width
toolconfig {{
  tools = List("rustfmt")
  rustfmt {{ skip = List("max_line_length") }}
}}
sections {{
  ["*.rs"] {{ max_line_length = width.width }}
}}
"#
        ),
    );
    let revision = fixture.commit();
    fixture.write(
        "project/.toolconfig.pkl",
        &format!(
            r#"amends "{package}"
root = true
toolconfig {{
  `extends` = List(new Repository {{
    git = "{URL}"
    rev = "{revision}"
  }})
  rustfmt {{ skip = List() }}
}}
"#
        ),
    );
    fixture.ok(&["generate"]);
    assert!(fixture.read("rustfmt.toml").contains("max_width = 96"));
    fixture.write(
        "project/.toolconfig.pkl",
        &format!(
            r#"amends "{package}"
root = true
toolconfig {{
  `extends` = List(new Repository {{
    git = "{URL}"
    rev = "{revision}"
  }})
  tools = List()
}}
"#
        ),
    );
    fixture.ok(&["generate"]);
    assert!(!fixture.path("project/rustfmt.toml").exists());
}

#[test]
fn corrupted_snapshot_is_rejected_before_publication() {
    let fixture = Fixture::new();
    fixture.write("source/.toolconfig.toml", SHARED);
    let revision = fixture.commit();
    fixture.configure(&revision, "");
    fixture.ok(&["generate"]);
    let before = fixture.read("rustfmt.toml");
    let snapshot = fs::read_dir(fixture.path("project/.repos"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.is_dir())
        .unwrap();
    fs::write(snapshot.join("tree/.toolconfig.toml"), "root = true\n").unwrap();
    assert!(
        fixture
            .fail(&["generate"])
            .contains("snapshot content changed")
    );
    assert_eq!(fixture.read("rustfmt.toml"), before);
}

#[test]
fn concurrent_projects_share_one_complete_snapshot() {
    let fixture = Fixture::new();
    fixture.write("source/.toolconfig.toml", SHARED);
    let revision = fixture.commit();
    fixture.configure(&revision, "");
    fixture.write("second/.toolconfig.toml", &fixture.read(".toolconfig.toml"));
    let first = fixture
        .command(&["generate"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let second = fixture
        .command(&["generate"])
        .current_dir(fixture.path("second"))
        .output()
        .unwrap();
    let first = first.wait_with_output().unwrap();
    for output in [first, second] {
        assert_success(output);
    }
    assert_eq!(
        fixture.read("rustfmt.toml"),
        fs::read_to_string(fixture.path("second/rustfmt.toml")).unwrap()
    );
}

fn assert_success(output: Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn public_http_transport_works_without_credentials() {
    let fixture = Fixture::new();
    fixture.write("source/.toolconfig.toml", SHARED);
    let revision = fixture.commit();
    let server = support::git_http::GitHttp::start(fixture.directory.path(), None);
    fixture.write("gitconfig", "");
    fixture.transport(&server.url());
    fixture.configure(&revision, "");
    fixture.ok(&["generate"]);
    assert!(fixture.path("project/rustfmt.toml").exists());
    assert_eq!(server.authenticated_requests(), 0);
}

#[test]
fn private_http_transport_uses_git_credential_helper_and_redacts_failures() {
    let fixture = Fixture::new();
    fixture.write("source/.toolconfig.toml", SHARED);
    let revision = fixture.commit();
    let server =
        support::git_http::GitHttp::start(fixture.directory.path(), Some("fixture-secret"));
    fixture.write("gitconfig", "");
    fixture.transport(&server.url());
    fixture.configure(&revision, "");
    let error = fixture.fail(&["generate"]);
    assert!(error.contains("credential helper or SSH"), "{error}");
    assert!(!error.contains("fixture-secret"), "{error}");
    fixture.git_config(
        "credential.helper",
        "!f() { printf '%s\\n' 'username=reader' 'password=fixture-secret'; }; f",
    );
    fixture.ok(&["generate"]);
    assert!(server.authenticated_requests() > 0);
    for entry in fs::read_dir(fixture.path("project/.repos")).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            assert!(
                !fs::read_to_string(path.join("manifest.json"))
                    .unwrap()
                    .contains("fixture-secret")
            );
            assert!(!path.join("objects").exists());
        }
    }
}

#[test]
fn ssh_imports_use_the_configured_git_ssh_command() {
    let fixture = Fixture::new();
    fixture.write("source/.toolconfig.toml", SHARED);
    let revision = fixture.commit();
    let quote = |path: PathBuf| {
        format!(
            "'{}'",
            path.to_string_lossy()
                .replace('\\', "/")
                .replace('\'', "'\\''")
        )
    };
    fixture.write(
        "ssh-helper.sh",
        &format!(
            "if [ \"$1\" = \"-G\" ]; then exit 0; fi\nprintf used > {}\nexec git upload-pack {}\n",
            quote(fixture.path("ssh-used")),
            quote(fixture.path("source/.git"))
        ),
    );
    for url in [
        "git@toolconfig.test:settings.git",
        "ssh://git@toolconfig.test/settings.git",
    ] {
        fixture.write(
            "project/.toolconfig.toml",
            &format!(
                "root = true\n[toolconfig]\nextends = [{{ git = {url:?}, rev = {revision:?} }}]\n"
            ),
        );
        let output = fixture
            .command(&["generate"])
            .env(
                "GIT_SSH_COMMAND",
                format!("sh {}", quote(fixture.path("ssh-helper.sh"))),
            )
            .env("GIT_SSH_VARIANT", "ssh")
            .output()
            .unwrap();
        assert_success(output);
    }
    assert!(fixture.path("ssh-used").exists());
}

#[test]
fn older_pkl_schema_defaults_remain_compatible() {
    let fixture = Fixture::new();
    fixture.write(
        "project/legacy.pkl",
        r#"module legacy
root: Boolean = false
toolconfig: Settings = new {}
sections: Mapping<String, Dynamic> = new {}
class Settings {
  tools: List<String> = List()
  rustfmt: ToolSettings = new {}
  ruff: ToolSettings = new {}
}
class ToolSettings {
  format: String?
  path: String?
  skip: List<String> = List()
}
"#,
    );
    fixture.write("project/.toolconfig.pkl", "amends \"legacy.pkl\"\nroot = true\ntoolconfig { tools = List(\"rustfmt\") }\nsections { [\"*.rs\"] { indent_size = 4 } }\n");
    fixture.ok(&["generate"]);
    assert!(fixture.read("rustfmt.toml").contains("tab_spaces = 4"));
    assert!(!fixture.path("project/ruff.toml").exists());
}

#[test]
fn root_discovery_does_not_inherit_repository_root() {
    let fixture = Fixture::new();
    fixture.write(
        "project/.toolconfig.toml",
        &format!(
            "[toolconfig]\nextends = [{{ git = {URL:?}, rev = {:?} }}]\n",
            "a".repeat(40)
        ),
    );
    assert!(
        fixture
            .fail(&["generate"])
            .contains("no ToolConfig source with root = true")
    );
    assert!(!fixture.path("project/.repos").exists());
}

#[test]
fn ambiguous_entrypoints_require_a_path() {
    let fixture = Fixture::new();
    fixture.write("source/.toolconfig.toml", SHARED);
    fixture.write("source/.editorconfig.toml", "[\"*\"]\nindent_size = 2\n");
    let revision = fixture.commit();
    fixture.configure(&revision, "");
    assert!(
        fixture
            .fail(&["validate"])
            .contains("multiple ToolConfig sources")
    );
    fixture.write("project/.toolconfig.toml", &format!("root = true\n[toolconfig]\nextends = [{{ git = {URL:?}, rev = {revision:?}, path = '.toolconfig.toml' }}]\n"));
    fixture.ok(&["generate"]);
    assert!(fixture.path("project/rustfmt.toml").exists());
}

#[test]
#[cfg(unix)] // Windows symlink creation requires privileges not present on all runners.
fn repository_symlinks_are_not_materialized() {
    let fixture = Fixture::new();
    fixture.write("source/real.toml", SHARED);
    std::os::unix::fs::symlink("real.toml", fixture.path("source/.toolconfig.toml")).unwrap();
    let revision = fixture.commit();
    fixture.configure(&revision, "");
    assert!(fixture.fail(&["generate"]).contains("no ToolConfig source"));
}

#[test]
fn native_tables_merge_arrays_replace_and_unset_clears() {
    let fixture = Fixture::new();
    fixture.write(
        "source/.toolconfig.toml",
        r#"[toolconfig]
tools = ["oxlint", "rustfmt"]
["*"]
indent_size = 4
oxlint.plugins = ["import", "typescript"]
oxlint.rules.eqeqeq = "error"
oxlint.rules.curly = "error"
"#,
    );
    let revision = fixture.commit();
    fixture.configure(
        &revision,
        r#"["*"]
indent_size = "unset"
oxlint.plugins = ["import"]
oxlint.rules.curly = "off"
"#,
    );
    fixture.ok(&["generate"]);
    let value: serde_json::Value = serde_json::from_str(&fixture.read(".oxlintrc.json")).unwrap();
    assert_eq!(value["plugins"], serde_json::json!(["import"]));
    assert_eq!(value["rules"]["eqeqeq"], "error");
    assert_eq!(value["rules"]["curly"], "off");
    let properties = ec4rs::properties_of(fixture.path("project/example.js")).unwrap();
    assert_eq!(
        properties
            .iter()
            .find(|(key, _)| *key == "indent_size")
            .unwrap()
            .1
            .into_str(),
        "unset"
    );
    assert!(!fixture.read("rustfmt.toml").contains("tab_spaces"));
}

#[test]
fn pinned_commit_changes_only_when_the_consumer_changes_rev() {
    let fixture = Fixture::new();
    fixture.write("source/.toolconfig.toml", SHARED);
    let old = fixture.commit();
    fixture.configure(&old, "");
    fixture.ok(&["generate"]);
    let original = fixture.read("rustfmt.toml");
    fixture.write(
        "source/.toolconfig.toml",
        &SHARED.replace("indent_size = 4", "indent_size = 8"),
    );
    let new = fixture.commit();
    fixture.ok(&["generate"]);
    assert_eq!(fixture.read("rustfmt.toml"), original);
    fixture.configure(&new, "");
    fixture.ok(&["generate"]);
    assert!(fixture.read("rustfmt.toml").contains("tab_spaces = 8"));
    fixture.configure(&old, "");
    fixture.ok(&["generate"]);
    assert_eq!(fixture.read("rustfmt.toml"), original);
}

#[test]
fn repository_cache_cannot_be_a_generated_output_or_project_root() {
    let fixture = Fixture::new();
    fixture.write(
        "source/.toolconfig.toml",
        "[toolconfig.files.bad]\npath = '.repos/overwrite'\nformat = 'text'\nconfig = 'bad'\n",
    );
    let revision = fixture.commit();
    fixture.configure(&revision, "");
    assert!(
        fixture
            .fail(&["generate"])
            .contains("collides with the repository cache")
    );
    assert!(!fixture.path("project/.repos/overwrite").exists());
    let output = fixture
        .command(&["generate"])
        .env("TOOLCONFIG_REPO_CACHE_DIR", fixture.path("project"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("must not contain the project root"));
}

#[test]
fn dynamic_pkl_import_cycles_report_the_chain() {
    let fixture = Fixture::new();
    let package = format!(
        "package://github.com/mathematic-inc/toolconfig/releases/download/v{0}/toolconfig@{0}#/ToolConfig.pkl",
        env!("CARGO_PKG_VERSION")
    );
    fixture.write(
        "source/.toolconfig.pkl",
        &format!(
            r#"amends "{package}"
toolconfig {{
  `extends` = List(new Repository {{
    git = "{URL}"
    rev = read("env:TOOLCONFIG_TEST_REV")
  }})
}}
"#
        ),
    );
    let revision = fixture.commit();
    fixture.configure(&revision, "");
    let output = fixture
        .command(&["generate"])
        .env("TOOLCONFIG_TEST_REV", &revision)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("repository import cycle"), "{error}");
    assert!(
        error.contains(&format!("{URL}@{revision}:.toolconfig.pkl ->")),
        "{error}"
    );
    assert!(!fixture.path("project/.editorconfig").exists());
}

#[test]
fn watch_ignores_repository_cache_and_tracks_local_pkl_inputs() {
    use std::io::{BufRead, BufReader};
    use std::process::{Child, Stdio};
    use std::sync::mpsc;
    use std::time::Duration;

    struct Running(Child);
    impl Running {
        fn shutdown(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    impl Drop for Running {
        fn drop(&mut self) {
            self.shutdown();
        }
    }

    let fixture = Fixture::new();
    let package = format!(
        "package://github.com/mathematic-inc/toolconfig/releases/download/v{0}/toolconfig@{0}#/ToolConfig.pkl",
        env!("CARGO_PKG_VERSION")
    );
    fixture.write("source/fragments/width.pkl", "width = 96\n");
    fixture.write(
        "source/.toolconfig.pkl",
        &format!(
            r#"amends "{package}"
import* "fragments/*.pkl" as fragments
toolconfig {{ tools = List("rustfmt") }}
sections {{ ["*.rs"] {{ max_line_length = fragments["fragments/width.pkl"].width }} }}
"#
        ),
    );
    let revision = fixture.commit();
    fixture.write("project/local/width.pkl", "width = 100\n");
    fixture.write(
        "project/.toolconfig.pkl",
        &format!(
            r#"amends "{package}"
import* "**/width.pkl" as fragments
root = true
toolconfig {{
  `extends` = List(new Repository {{
    git = "{URL}"
    rev = "{revision}"
  }})
}}
sections {{ ["*.rs"] {{ max_line_length = fragments["local/width.pkl"].width }} }}
"#
        ),
    );
    let mut running = Running(
        fixture
            .command(&["generate", "--watch"])
            .env("TOOLCONFIG_WATCH_TRACE", "1")
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let stderr = running.0.stderr.take().unwrap();
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            let _ = sender.send(line);
        }
    });
    let generated = || {
        loop {
            let line = receiver
                .recv_timeout(Duration::from_secs(20))
                .expect("watch should generate");
            if line.contains("generated") {
                break;
            }
        }
    };
    generated();
    assert!(fixture.read("rustfmt.toml").contains("max_width = 100"));
    fixture.write("project/.repos/.fetch-abandoned/width.pkl", "width = 1\n");
    fixture.write(
        "project/.repos/.fetch-abandoned/.toolconfig.toml",
        "root = true\n",
    );
    assert!(
        receiver.recv_timeout(Duration::from_millis(1500)).is_err(),
        "repository cache activity triggered watch mode"
    );
    fixture.write("project/local/width.pkl", "width = 120\n");
    generated();
    assert!(fixture.read("rustfmt.toml").contains("max_width = 120"));
    assert!(
        receiver.recv_timeout(Duration::from_millis(1500)).is_err(),
        "watch mode regenerated in a loop"
    );
}
