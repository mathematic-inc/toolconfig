//! Vale reads more than `.vale.ini`: its styles and vocabularies are folders of files under the
//! `StylesPath`. ToolConfig writes the bundled styles a source lists, and the vocabularies it
//! declares, beside the INI file.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::model::{self, Origin};
use crate::paths;
use crate::value::Value;

/// Where styles go when a source sets no `StylesPath`.
pub(crate) const STYLES_PATH: &str = ".vale/styles";

/// A style ToolConfig carries: a folder of Vale rule files.
pub(crate) struct Style {
    pub(crate) name: &'static str,
    /// Each rule's name, which is its file's name without `.yml`, and the file's text.
    pub(crate) rules: &'static [(&'static str, &'static str)],
}

/// STE checks the part of Simplified Technical English that a linter can: punctuation, tenses,
/// -ing forms, the passive voice, noun clusters and the length of a sentence. Its rules take the
/// terms a source accepts, and the ones it rejects, from the source's vocabularies.
pub(crate) const STYLES: &[Style] = &[Style {
    name: "STE",
    rules: &[
        ("Contractions", include_str!("vale/ste/Contractions.yml")),
        ("Ing", include_str!("vale/ste/Ing.yml")),
        (
            "LatinAbbreviations",
            include_str!("vale/ste/LatinAbbreviations.yml"),
        ),
        ("NounClusters", include_str!("vale/ste/NounClusters.yml")),
        ("Passive", include_str!("vale/ste/Passive.yml")),
        ("PerfectTense", include_str!("vale/ste/PerfectTense.yml")),
        (
            "ProgressiveTense",
            include_str!("vale/ste/ProgressiveTense.yml"),
        ),
        ("Semicolons", include_str!("vale/ste/Semicolons.yml")),
        (
            "SentenceLength",
            include_str!("vale/ste/SentenceLength.yml"),
        ),
        ("StepLength", include_str!("vale/ste/StepLength.yml")),
    ],
}];

pub(crate) fn style(name: &str) -> Option<&'static Style> {
    STYLES.iter().find(|style| style.name == name)
}

/// What `[toolconfig.vale]` holds besides the settings every tool has.
#[derive(Clone, Debug, Default)]
pub(crate) struct Settings {
    /// The bundled styles to write. `None` when the source says nothing, which keeps the list an
    /// imported source chose.
    pub(crate) styles: Option<Vec<String>>,
    pub(crate) vocabularies: Vec<Vocabulary>,
}

/// A folder under `StylesPath/config/vocabularies`: Vale's list of terms.
#[derive(Clone, Debug)]
pub(crate) struct Vocabulary {
    pub(crate) name: String,
    /// Regular expressions, one per line of `accept.txt`: terms with a required spelling, which
    /// every rule of every style ignores.
    pub(crate) accept: Vec<String>,
    /// Regular expressions, one per line of `reject.txt`, which Vale reports as errors.
    pub(crate) reject: Vec<String>,
}

/// A file written beside the INI file.
pub(crate) struct File {
    pub(crate) path: PathBuf,
    /// What produced it: `vale.style.<name>` or `vale.vocabulary.<name>`.
    pub(crate) label: String,
    pub(crate) format: &'static str,
    pub(crate) bytes: Vec<u8>,
}

impl Settings {
    pub(crate) fn is_empty(&self) -> bool {
        self.styles.is_none() && self.vocabularies.is_empty()
    }

    /// Reads the two keys from either source syntax. `origin` names the source in errors.
    pub(crate) fn parse(
        styles: Option<&Value>,
        vocabularies: Option<&Value>,
        origin: &Origin,
    ) -> Result<Self> {
        let styles = match styles {
            None | Some(Value::Null) => None,
            Some(Value::Array(items)) => Some(
                items
                    .iter()
                    .map(|item| {
                        item.as_str().map(str::to_owned).with_context(|| {
                            format!(
                                "{origin}: vale styles must be names, not {}",
                                item.describe()
                            )
                        })
                    })
                    .collect::<Result<_>>()?,
            ),
            Some(other) => bail!(
                "{origin}: vale styles must be a list of names, not {}",
                other.describe()
            ),
        };
        let vocabularies = match vocabularies {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Object(fields)) => fields
                .iter()
                .map(|(name, value)| Vocabulary::parse(name, value, origin))
                .collect::<Result<_>>()?,
            Some(other) => bail!(
                "{origin}: vale vocabularies must be a table of vocabularies, not {}",
                other.describe()
            ),
        };
        Ok(Self {
            styles,
            vocabularies,
        })
    }

    /// Lays an imported source's settings under this one's: a list of styles replaces, and a
    /// vocabulary replaces the one of its name.
    pub(crate) fn apply(&mut self, incoming: Self) {
        if incoming.styles.is_some() {
            self.styles = incoming.styles;
        }
        for vocabulary in incoming.vocabularies {
            match self
                .vocabularies
                .iter_mut()
                .find(|existing| existing.name == vocabulary.name)
            {
                Some(existing) => *existing = vocabulary,
                None => self.vocabularies.push(vocabulary),
            }
        }
    }

    pub(crate) fn validate(&self, source: &Path) -> Result<()> {
        let mut seen = BTreeSet::new();
        for name in self.styles.iter().flatten() {
            if style(name).is_none() {
                let hint = model::suggestion(name, STYLES.iter().map(|style| style.name));
                let styles = STYLES.iter().map(|style| style.name).collect::<Vec<_>>();
                bail!(
                    "{}: unknown vale style {name:?}{}",
                    source.display(),
                    if hint.is_empty() {
                        format!("; ToolConfig writes {}", styles.join(", "))
                    } else {
                        hint
                    }
                );
            }
            if !seen.insert(name) {
                bail!("{}: vale lists style {name} twice", source.display());
            }
        }
        for vocabulary in &self.vocabularies {
            vocabulary.validate(source)?;
        }
        Ok(())
    }
}

/// Vale reads a style or a vocabulary by its folder's name.
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.starts_with(|ch: char| ch.is_ascii_alphanumeric())
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
}

impl Vocabulary {
    fn parse(name: &str, value: &Value, origin: &Origin) -> Result<Self> {
        let Some(fields) = value.as_object() else {
            bail!(
                "{origin}: vale vocabulary {name:?} must be a table with accept and reject lists, not {}",
                value.describe()
            );
        };
        let mut vocabulary = Self {
            name: name.to_owned(),
            accept: Vec::new(),
            reject: Vec::new(),
        };
        for (key, value) in fields {
            let list = match key.as_str() {
                "accept" => &mut vocabulary.accept,
                "reject" => &mut vocabulary.reject,
                other => bail!(
                    "{origin}: unknown key {other:?} in vale vocabulary {name:?}; expected accept or reject"
                ),
            };
            *list = match value {
                Value::Null => Vec::new(),
                Value::Array(items) => items
                    .iter()
                    .map(|item| {
                        item.as_str().map(str::to_owned).with_context(|| {
                            format!(
                                "{origin}: vale vocabulary {name:?} {key} must list strings, not {}",
                                item.describe()
                            )
                        })
                    })
                    .collect::<Result<_>>()?,
                other => bail!(
                    "{origin}: vale vocabulary {name:?} {key} must be a list of strings, not {}",
                    other.describe()
                ),
            };
        }
        Ok(vocabulary)
    }

    fn validate(&self, source: &Path) -> Result<()> {
        let name = &self.name;
        if !valid_name(name) {
            bail!(
                "{}: vale vocabulary {name:?} is a folder's name: use letters, digits, - and _",
                source.display()
            );
        }
        if self.accept.is_empty() && self.reject.is_empty() {
            bail!(
                "{}: vale vocabulary {name} has no accept or reject entries",
                source.display()
            );
        }
        for (list, entries) in [("accept", &self.accept), ("reject", &self.reject)] {
            for entry in entries {
                if entry.is_empty()
                    || entry.trim() != entry
                    || entry.contains(['\n', '\r', '\0'])
                    || entry.starts_with("# ")
                {
                    bail!(
                        "{}: vale vocabulary {name} {list} entry {entry:?} must be one line of text with no surrounding whitespace, and not start with \"# \", which Vale reads as a comment",
                        source.display()
                    );
                }
            }
        }
        Ok(())
    }
}

/// The files for `settings`, under the `StylesPath` that `native` (the INI file's content) chooses.
/// `ini` is where the INI file goes: Vale reads a relative `StylesPath` from its folder.
pub(crate) fn files(settings: &Settings, native: &Value, ini: &Path) -> Result<Vec<File>> {
    if settings.is_empty() {
        return Ok(Vec::new());
    }
    let styles_path = match native.get("StylesPath") {
        Some(Value::String(path)) => path.as_str(),
        Some(other) => bail!("StylesPath must be a path, not {}", other.describe()),
        None => STYLES_PATH,
    };
    let folder = ini.parent().context("the INI file needs a folder")?;
    let styles = paths::absolute(folder, Path::new(styles_path))?;
    let mut files = Vec::new();
    for name in settings.styles.iter().flatten() {
        let style = style(name).with_context(|| format!("unknown vale style {name:?}"))?;
        for (rule, text) in style.rules {
            files.push(File {
                path: styles.join(name).join(format!("{rule}.yml")),
                label: format!("vale.style.{name}"),
                format: "yaml",
                bytes: text.as_bytes().to_vec(),
            });
        }
    }
    for vocabulary in &settings.vocabularies {
        let folder = styles
            .join("config")
            .join("vocabularies")
            .join(&vocabulary.name);
        for (file, entries) in [
            ("accept.txt", &vocabulary.accept),
            ("reject.txt", &vocabulary.reject),
        ] {
            if entries.is_empty() {
                continue;
            }
            let mut text = entries.join("\n");
            text.push('\n');
            files.push(File {
                path: folder.join(file),
                label: format!("vale.vocabulary.{}", vocabulary.name),
                format: "lines",
                bytes: text.into_bytes(),
            });
        }
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn origin() -> Origin {
        Origin::new(Path::new(".toolconfig.toml"), None)
    }

    fn json(value: serde_json::Value) -> Value {
        Value::from_json(&value)
    }

    #[test]
    fn every_rule_file_is_bundled() {
        let folder = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/vale/ste");
        let mut on_disk = std::fs::read_dir(folder)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect::<Vec<_>>();
        on_disk.sort();
        let bundled = style("STE")
            .unwrap()
            .rules
            .iter()
            .map(|(rule, _)| format!("{rule}.yml"))
            .collect::<Vec<_>>();
        assert_eq!(on_disk, bundled, "list each rule file in STYLES, in order");
    }

    #[test]
    fn every_rule_names_a_style_it_belongs_to_and_a_level() {
        for style in STYLES {
            assert!(valid_name(style.name));
            for (rule, text) in style.rules {
                assert!(valid_name(rule), "{rule}");
                assert!(text.contains("\nextends: "), "{rule} extends a check");
                assert!(text.contains("\nmessage: "), "{rule} has a message");
                assert!(text.contains("\nlevel: "), "{rule} has a level");
            }
        }
    }

    #[test]
    fn parses_styles_and_vocabularies() {
        let settings = Settings::parse(
            Some(&json(serde_json::json!(["STE"]))),
            Some(&json(serde_json::json!({
                "Terms": {"accept": ["(?i)runner"], "reject": ["utilize"]},
                "Other": {"accept": ["x"]}
            }))),
            &origin(),
        )
        .unwrap();
        assert_eq!(settings.styles.as_deref(), Some(&["STE".to_owned()][..]));
        assert_eq!(settings.vocabularies.len(), 2);
        assert_eq!(settings.vocabularies[1].reject, Vec::<String>::new());
        settings.validate(Path::new(".toolconfig.toml")).unwrap();
        for invalid in [
            (Some(serde_json::json!("STE")), None),
            (Some(serde_json::json!([1])), None),
            (None, Some(serde_json::json!(["a"]))),
            (None, Some(serde_json::json!({"A": ["a"]}))),
            (None, Some(serde_json::json!({"A": {"accept": "a"}}))),
            (None, Some(serde_json::json!({"A": {"allow": ["a"]}}))),
        ] {
            let (styles, vocabularies) = invalid;
            let shown = format!("{styles:?} {vocabularies:?}");
            let error = Settings::parse(
                styles.map(json).as_ref(),
                vocabularies.map(json).as_ref(),
                &origin(),
            );
            assert!(error.is_err(), "{shown}");
        }
    }

    #[test]
    fn validation_names_the_problem() {
        let check = |styles: serde_json::Value, vocabularies: serde_json::Value| {
            Settings::parse(Some(&json(styles)), Some(&json(vocabularies)), &origin())
                .unwrap()
                .validate(Path::new(".toolconfig.toml"))
                .unwrap_err()
                .to_string()
        };
        let none = serde_json::json!({});
        assert!(check(serde_json::json!(["STEE"]), none.clone()).contains("did you mean \"STE\"?"));
        assert!(check(serde_json::json!(["STE", "STE"]), none.clone()).contains("twice"));
        assert!(
            check(
                serde_json::json!([]),
                serde_json::json!({"a/b": {"accept": ["x"]}})
            )
            .contains("folder's name")
        );
        assert!(
            check(
                serde_json::json!([]),
                serde_json::json!({"A": {"accept": []}})
            )
            .contains("no accept or reject entries")
        );
        for entry in ["", " x", "# note", "a\nb"] {
            assert!(
                check(
                    serde_json::json!([]),
                    serde_json::json!({"A": {"reject": [entry]}})
                )
                .contains("one line of text"),
                "{entry:?}"
            );
        }
    }

    #[test]
    fn imports_lay_under_the_importing_source() {
        let mut base = Settings::parse(
            Some(&json(serde_json::json!(["STE"]))),
            Some(&json(serde_json::json!({
                "A": {"accept": ["a"]},
                "B": {"accept": ["b"]}
            }))),
            &origin(),
        )
        .unwrap();
        let incoming = Settings::parse(
            None,
            Some(&json(serde_json::json!({
                "B": {"reject": ["c"]},
                "C": {"accept": ["d"]}
            }))),
            &origin(),
        )
        .unwrap();
        base.apply(incoming);
        assert_eq!(base.styles.as_deref(), Some(&["STE".to_owned()][..]));
        let names = base
            .vocabularies
            .iter()
            .map(|vocabulary| (vocabulary.name.as_str(), vocabulary.accept.len()))
            .collect::<Vec<_>>();
        assert_eq!(names, [("A", 1), ("B", 0), ("C", 1)]);
    }

    #[test]
    fn files_go_under_the_styles_path_beside_the_ini_file() {
        let settings = Settings::parse(
            Some(&json(serde_json::json!(["STE"]))),
            Some(&json(serde_json::json!({"Terms": {"accept": ["a", "b"]}}))),
            &origin(),
        )
        .unwrap();
        let project = Path::new(env!("CARGO_MANIFEST_DIR"));
        let docs = project.join("docs");
        let native = json(serde_json::json!({"StylesPath": "lint/styles"}));
        let written = files(&settings, &native, &docs.join(".vale.ini")).unwrap();
        let paths = written
            .iter()
            .map(|file| file.path.strip_prefix(&docs).unwrap().to_owned())
            .collect::<Vec<_>>();
        assert!(paths.contains(&PathBuf::from("lint/styles/STE/Ing.yml")));
        let vocabulary = written.last().unwrap();
        assert_eq!(
            vocabulary.path,
            docs.join("lint/styles/config/vocabularies/Terms/accept.txt")
        );
        assert_eq!(vocabulary.bytes, b"a\nb\n");
        assert_eq!(written.len(), STYLES[0].rules.len() + 1);
        let default = files(&settings, &Value::object(), &project.join(".vale.ini")).unwrap();
        assert!(
            default[0]
                .path
                .starts_with(project.join(".vale/styles/STE"))
        );
    }
}
