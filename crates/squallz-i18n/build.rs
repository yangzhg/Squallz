use std::env;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

fn main() -> Result<(), Box<dyn Error>> {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
    let locales_dir = manifest_dir.join("../../locales");
    println!("cargo:rerun-if-changed={}", locales_dir.display());

    let mut entries = Vec::new();
    for entry in fs::read_dir(&locales_dir)? {
        let path = entry?.path();
        let Some(tag) = locale_tag_of(&path) else {
            continue;
        };
        println!("cargo:rerun-if-changed={}", path.display());
        entries.push((tag, path));
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));

    if !entries.iter().any(|(tag, _)| tag == "en-US") {
        return Err(format!(
            "fallback language pack en-US.json is missing from {}",
            locales_dir.display()
        )
        .into());
    }

    let mut generated = String::from("const BUILTIN: &[(&str, &str)] = &[\n");
    for (tag, path) in entries {
        generated.push_str("    (");
        generated.push_str(&rust_string(&tag));
        generated.push_str(", include_str!(");
        generated.push_str(&rust_string(&path.to_string_lossy()));
        generated.push_str(")),\n");
    }
    generated.push_str("];\n");

    let out_dir = PathBuf::from(env::var("OUT_DIR")?);
    fs::write(out_dir.join("builtin_locales.rs"), generated)?;
    Ok(())
}

fn locale_tag_of(path: &Path) -> Option<String> {
    if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
        return None;
    }
    let stem = path.file_stem()?.to_str()?;
    let valid = !stem.is_empty() && stem.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    valid.then(|| stem.to_owned())
}

fn rust_string(value: &str) -> String {
    let mut out = String::from("\"");
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
