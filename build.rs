use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=src/catalog.json");
    let catalog: serde_json::Value = serde_json::from_slice(&std::fs::read("src/catalog.json")?)?;
    let ids = catalog
        .as_array()
        .ok_or("catalog must be an array")?
        .iter()
        .map(|entry| entry["id"].as_str().map(str::to_owned))
        .collect::<Option<Vec<_>>>()
        .ok_or("catalog entry needs id")?;
    let module = pkl_module(&ids);
    let out = PathBuf::from(std::env::var_os("OUT_DIR").ok_or("OUT_DIR is missing")?);
    std::fs::write(out.join("ToolConfig.pkl"), &module)?;
    let archive = std::fs::File::create(out.join("toolconfig-package.zip"))?;
    let mut zip = zip::ZipWriter::new(archive);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .system(zip::System::Unix)
        .last_modified_time(zip::DateTime::default())
        .unix_permissions(0o644);
    zip.start_file("ToolConfig.pkl", options)?;
    zip.write_all(module.as_bytes())?;
    zip.finish()?;
    let version = std::env::var("CARGO_PKG_VERSION")?;
    let name = format!("toolconfig@{version}");
    let uri = format!(
        "package://github.com/mathematic-inc/toolconfig/releases/download/v{version}/{name}"
    );
    let bytes = std::fs::read(out.join("toolconfig-package.zip"))?;
    let metadata = serde_json::json!({
        "name": "toolconfig",
        "packageUri": uri,
        "version": version,
        "packageZipUrl": format!("https://github.com/mathematic-inc/toolconfig/releases/download/v{version}/{name}.zip"),
        "packageZipChecksums": { "sha256": hex(&Sha256::digest(&bytes)) },
        "dependencies": {},
        "license": "MIT",
        "sourceCode": "https://github.com/mathematic-inc/toolconfig",
        "issueTracker": "https://github.com/mathematic-inc/toolconfig/issues"
    });
    let metadata = serde_json::to_vec_pretty(&metadata)?;
    std::fs::write(out.join(&name), &metadata)?;
    std::fs::write(
        out.join(format!("{name}.sha256")),
        hex(&Sha256::digest(&metadata)),
    )?;
    std::fs::write(out.join(format!("{name}.zip")), bytes)?;
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn pkl_module(ids: &[String]) -> String {
    let mut settings = String::new();
    let mut tables = String::new();
    for id in ids {
        settings.push_str(&format!(
            "  /// How {id} is generated.\n  `{id}`: ToolSettings = new {{}}\n"
        ));
        tables.push_str(&format!(
            "  /// Native {id} options for the files this section matches.\n  `{id}`: Dynamic = new {{}}\n"
        ));
    }
    format!(
        r#"/// ToolConfig sources: EditorConfig sections with native options for each tool.
///
/// Amend this module in `.toolconfig.pkl`. ToolConfig generates `.editorconfig` and each
/// listed tool's configuration file from it. Generated from the tool catalog.
module toolconfig.ToolConfig

/// Stops the EditorConfig cascade at this directory. The project's top source must set it.
root: Boolean = false

/// Settings for the files generated in this directory.
toolconfig: Settings = new {{}}

/// EditorConfig sections keyed by glob. Later sections take precedence.
sections: Mapping<String, Section> = new {{}}

class Settings {{
  /// Tools whose native configuration file is generated in this directory.
  tools: List<String> = List()
  /// Additional generated files, keyed by label.
  files: Mapping<String, File> = new {{}}
{settings}}}

class ToolSettings {{
  /// The native format to write.
  format: String?
  /// The native filename to write, among those the tool reads.
  path: String?
  /// Standard properties not projected into this tool; "*" skips all.
  skip: List<String> = List()
}}

class File {{
  /// Path relative to this directory.
  path: String
  /// json, yaml, toml, ini, text, or lines.
  format: String = "json"
  /// The file's content: an object, a String for text, or a List of Strings for lines.
  config: Any = new Dynamic {{}}
}}

open class Section {{
  indent_style: ("space"|"tab"|"unset")?
  indent_size: (Int|"tab"|"unset")?
  tab_width: (Int|"unset")?
  end_of_line: ("lf"|"crlf"|"cr"|"unset")?
  charset: ("latin1"|"utf-8"|"utf-8-bom"|"utf-16be"|"utf-16le"|"unset")?
  trim_trailing_whitespace: (Boolean|"unset")?
  insert_final_newline: (Boolean|"unset")?
  max_line_length: (Int|"off"|"unset")?
  spelling_language: String?
  /// Other EditorConfig properties, written to `.editorconfig` as is.
  properties: Mapping<String, String|Int|Float|Boolean> = new {{}}
{tables}}}
"#
    )
}
