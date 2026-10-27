//! Pkl sources: `.toolconfig.pkl`, evaluated by the embedded pklr with recorded inputs.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, bail};
use pklr::{EvalCapabilities, Evaluator, FetchBudget, NativeCapabilities};

use crate::catalog;
use crate::inputs::{Inputs, digest, within};
use crate::model::{
    FileSpec, Origin, Property, Section, Settings, Source, ToolSettings, ToolTable,
};
use crate::paths::absolute;
use crate::value::Value;

const EMBEDDED_PACKAGE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/toolconfig-package.zip"));
const VERSION: &str = env!("CARGO_PKG_VERSION");

pub(crate) fn package_uri() -> String {
    format!(
        "package://github.com/mathematic-inc/toolconfig/releases/download/v{VERSION}/toolconfig@{VERSION}"
    )
}

/// Evaluates a Pkl source. Inputs are returned even when evaluation fails, so watch mode
/// can wait for the files involved to change.
pub(crate) fn evaluate(path: &Path, root: &Path) -> (Result<Source>, Inputs) {
    let recorded = Arc::new(Mutex::new(Inputs::default()));
    let source = (|| -> Result<Source> {
        let dir = path.parent().context("a source needs a directory")?;
        let cache = std::env::var_os("TOOLCONFIG_PKL_CACHE_DIR")
            .map(PathBuf::from)
            .or_else(|| dirs::cache_dir().map(|path| path.join("toolconfig/pkl")))
            .context("cannot locate a package cache; set TOOLCONFIG_PKL_CACHE_DIR")?;
        // A development rebuild must not reuse different package bytes under the same version.
        let cache = absolute(root, &cache)?.join(digest(EMBEDDED_PACKAGE));
        std::fs::create_dir_all(&cache).context("cannot create the Pkl package cache")?;
        let cache = cache
            .canonicalize()
            .context("cannot resolve the package cache")?;
        recorded
            .lock()
            .map_err(|_| anyhow::anyhow!("input tracking failed"))?
            .excluded
            .insert(cache.clone());
        let capabilities = Tracking {
            inner: NativeCapabilities::new(),
            recorded: Arc::clone(&recorded),
            root: root.to_path_buf(),
            source_dir: dir.to_path_buf(),
            cache: cache.clone(),
            temporary: Vec::new(),
        };
        let mut evaluator = Evaluator::with_capabilities(capabilities);
        evaluator.set_package_cache_dir(cache);
        evaluator.set_offline(env_flag("TOOLCONFIG_PKL_OFFLINE")?);
        let archive = format!(
            "https://github.com/mathematic-inc/toolconfig/releases/download/v{VERSION}/toolconfig@{VERSION}.zip"
        );
        evaluator
            .preload_package(&archive, "zip", EMBEDDED_PACKAGE)
            .context("cannot load the embedded Pkl package")?;
        evaluator.set_base_path(dir);
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| evaluator.eval_file(path)));
        let raw = result
            .map_err(|_| {
                anyhow::anyhow!(
                    "the Pkl evaluator panicked while evaluating {}; no outputs were written",
                    path.display()
                )
            })?
            .map_err(pkl_error)
            .with_context(|| format!("cannot evaluate {}", path.display()))?;
        let value = Value::from_pkl(&raw, "module")?;
        convert(path, dir, &value)
    })();
    let inputs = match recorded.lock() {
        Ok(inputs) => inputs.clone(),
        Err(_) => Inputs::default(),
    };
    (source, inputs)
}

fn convert(path: &Path, dir: &Path, value: &Value) -> Result<Source> {
    let origin = Origin::new(path, None);
    let mut source = Source {
        path: path.to_path_buf(),
        dir: dir.to_path_buf(),
        root: false,
        settings: Settings::default(),
        sections: Vec::new(),
    };
    let fields = value
        .as_object()
        .with_context(|| format!("{origin}: the module must be an object"))?;
    for (key, value) in fields {
        match key.as_str() {
            "root" => {
                source.root = match value {
                    Value::Bool(value) => *value,
                    Value::Null => false,
                    _ => bail!("{origin}: root must be true or false"),
                };
            }
            "toolconfig" => source.settings = settings(&origin, value)?,
            "sections" => {
                for (glob, value) in value
                    .as_object()
                    .with_context(|| format!("{origin}: sections must be a mapping"))?
                {
                    source.sections.push(section(&origin, glob, value)?);
                }
            }
            other => bail!(
                "{origin}: unknown module property {other:?}; amend the ToolConfig module and set root, toolconfig or sections"
            ),
        }
    }
    source.validate()?;
    Ok(source)
}

fn settings(origin: &Origin, value: &Value) -> Result<Settings> {
    let mut settings = Settings::default();
    let Some(fields) = value.as_object() else {
        bail!("{origin}: toolconfig must be an object");
    };
    for (key, value) in fields {
        match key.as_str() {
            "tools" => match value {
                Value::Array(values) => {
                    for value in values {
                        let id = value
                            .as_str()
                            .with_context(|| format!("{origin}: tools must list tool names"))?;
                        settings.tools.push((id.to_owned(), origin.clone()));
                    }
                }
                Value::Null => {}
                _ => bail!("{origin}: tools must be a List of tool names"),
            },
            "files" => {
                for (label, file) in value.as_object().unwrap_or_default() {
                    let text = |name: &str| file.get(name).and_then(Value::as_str);
                    settings.files.push(FileSpec {
                        label: label.clone(),
                        path: text("path")
                            .with_context(|| format!("{origin}: file {label:?} needs a path"))?
                            .to_owned(),
                        format: text("format").unwrap_or("json").to_owned(),
                        config: file.get("config").cloned().unwrap_or_else(Value::object),
                        origin: origin.clone(),
                    });
                }
            }
            id => {
                if value.is_empty_object() || matches!(value, Value::Null) {
                    continue;
                }
                let text = |name: &str| value.get(name).and_then(Value::as_str).map(str::to_owned);
                let skip = match value.get("skip") {
                    Some(Value::Array(values)) => values
                        .iter()
                        .map(|value| {
                            value.as_str().map(str::to_owned).with_context(|| {
                                format!("{origin}: {id}.skip must list property names")
                            })
                        })
                        .collect::<Result<_>>()?,
                    _ => Vec::new(),
                };
                let tool = ToolSettings {
                    format: text("format"),
                    path: text("path"),
                    skip,
                };
                if tool.format.is_some() || tool.path.is_some() || !tool.skip.is_empty() {
                    settings.tool.insert(id.to_owned(), tool);
                }
            }
        }
    }
    Ok(settings)
}

fn section(origin: &Origin, glob: &str, value: &Value) -> Result<Section> {
    let mut section = Section::new(glob.to_owned(), origin.clone())?;
    let fields = value
        .as_object()
        .with_context(|| format!("{origin}: section {glob:?} must be an object"))?;
    for (key, value) in fields {
        if key == "properties" {
            for (key, value) in value.as_object().unwrap_or_default() {
                section.properties.push(Property {
                    key: key.clone(),
                    value: value.clone(),
                    origin: origin.clone(),
                });
            }
        } else if catalog::get(key).is_some() {
            if !value.is_empty_object() && !matches!(value, Value::Null) {
                section.tools.push(ToolTable {
                    id: key.clone(),
                    value: value.clone(),
                    origins: Vec::new(),
                });
            }
        } else if !matches!(value, Value::Null) {
            section.properties.push(Property {
                key: key.clone(),
                value: value.clone(),
                origin: origin.clone(),
            });
        }
    }
    Ok(section)
}

fn env_flag(name: &str) -> Result<bool> {
    match std::env::var(name).as_deref() {
        Ok("1" | "true") => Ok(true),
        Ok("0" | "false" | "") | Err(std::env::VarError::NotPresent) => Ok(false),
        _ => bail!("{name} must be true, false, 1, or 0"),
    }
}

struct Tracking {
    inner: NativeCapabilities,
    recorded: Arc<Mutex<Inputs>>,
    root: PathBuf,
    source_dir: PathBuf,
    cache: PathBuf,
    temporary: Vec<tempfile::TempDir>,
}

impl Tracking {
    fn record(&self, path: &Path, bytes: Option<&[u8]>) -> pklr::Result<()> {
        if within(path, &self.cache) {
            return Ok(());
        }
        let mut inputs = self
            .recorded
            .lock()
            .map_err(|_| pklr::Error::Unsupported("input tracking failed".into()))?;
        let hash = bytes.map(digest);
        inputs.files.insert(path.to_path_buf(), hash.clone());
        if let Ok(real) = path.canonicalize() {
            inputs.files.insert(real, hash);
        }
        Ok(())
    }
}

impl EvalCapabilities for Tracking {
    fn is_native_filesystem(&self) -> bool {
        true
    }
    fn read_to_string(&mut self, path: &Path) -> pklr::Result<String> {
        let result = self.inner.read_to_string(path);
        self.record(path, result.as_ref().ok().map(String::as_bytes))?;
        result
    }
    fn read_bytes(&mut self, path: &Path) -> pklr::Result<Vec<u8>> {
        let result = self.inner.read_bytes(path);
        self.record(path, result.as_ref().ok().map(Vec::as_slice))?;
        result
    }
    fn path_exists(&mut self, path: &Path) -> pklr::Result<bool> {
        let exists = self.inner.path_exists(path)?;
        if !exists {
            self.record(path, None)?;
        }
        Ok(exists)
    }
    fn canonicalize(&mut self, path: &Path) -> pklr::Result<PathBuf> {
        self.inner.canonicalize(path)
    }
    fn read_env(&mut self, name: &str) -> pklr::Result<Option<String>> {
        match name {
            "TOOLCONFIG_ROOT_DIR" => Ok(Some(self.root.to_string_lossy().into_owned())),
            "TOOLCONFIG_SOURCE_DIR" => Ok(Some(self.source_dir.to_string_lossy().into_owned())),
            _ => self.inner.read_env(name),
        }
    }
    fn fetch_text(&mut self, url: &str) -> pklr::Result<String> {
        self.inner.fetch_text(url)
    }
    fn fetch_bytes(&mut self, url: &str) -> pklr::Result<Vec<u8>> {
        self.inner.fetch_bytes(url)
    }
    fn fetch_text_many(
        &mut self,
        urls: &[String],
        budget: &FetchBudget,
    ) -> Vec<pklr::Result<String>> {
        self.inner.fetch_text_many(urls, budget)
    }
    fn fetch_bytes_many(
        &mut self,
        urls: &[String],
        budget: &FetchBudget,
    ) -> Vec<pklr::Result<Vec<u8>>> {
        self.inner.fetch_bytes_many(urls, budget)
    }
    fn temp_dir(&mut self, prefix: &str) -> pklr::Result<PathBuf> {
        let directory = tempfile::Builder::new()
            .prefix(prefix)
            .tempdir_in(&self.cache)
            .map_err(|error| pklr::Error::Io(self.cache.clone(), error))?;
        let path = directory.path().to_path_buf();
        self.temporary.push(directory);
        Ok(path)
    }
    fn glob(&mut self, base: &Path, pattern: &str) -> pklr::Result<Vec<PathBuf>> {
        let paths = self
            .inner
            .glob(base, pattern)?
            .into_iter()
            .filter(|path| !within(path, &self.cache))
            .collect::<Vec<_>>();
        let mut recorded = self
            .recorded
            .lock()
            .map_err(|_| pklr::Error::Unsupported("input tracking failed".into()))?;
        recorded.directories.insert(base.to_path_buf());
        recorded
            .globs
            .insert((base.to_path_buf(), pattern.to_owned()), paths.clone());
        Ok(paths)
    }
}

fn pkl_error(error: pklr::Error) -> anyhow::Error {
    match &error {
        pklr::Error::Parse { src, span, .. } | pklr::Error::Lex { src, span, .. } => {
            let prefix = src.inner().get(..span.offset()).unwrap_or("");
            let line = prefix.bytes().filter(|byte| *byte == b'\n').count() + 1;
            let column = prefix.rsplit('\n').next().unwrap_or("").chars().count() + 1;
            anyhow::anyhow!("{}:{line}:{column}: {error}", src.name())
        }
        _ => anyhow::Error::new(error),
    }
}
