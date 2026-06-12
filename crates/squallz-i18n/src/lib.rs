#![forbid(unsafe_code)]
//! squallz-i18n: language-pack lookup shared by the CLI and the GUI
//! (PLAN.md §5.5: a language is a configuration file).
//!
//! - Built-in packs (`locales/<BCP47>.json` at the repository root) are
//!   embedded at compile time; the GUI ships the same files as resources.
//! - At runtime the user locale directory is scanned
//!   (macOS: `~/Library/Application Support/Squallz/locales/*.json`):
//!   same-named keys override the built-ins, a new BCP 47 file name adds a
//!   whole new language.
//! - Lookup falls back per key: selected language → en-US → the bare key
//!   (with a debug log entry).
//! - Templates use simple `{name}` placeholders substituted by string
//!   replacement.

mod errors;

pub use errors::{error_message, localize_error, ErrorMessage};

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The last language in the fallback chain; its built-in pack defines the
/// complete key set.
pub const FALLBACK_LANG: &str = "en-US";

/// Environment variable selecting the language (lower priority than an
/// explicit `--lang`, higher than the system locale).
pub const LANG_ENV: &str = "SQZ_LANG";

/// Environment variable overriding the user locale directory (used by tests
/// and power users; defaults to the platform data dir + `Squallz/locales`).
pub const LOCALES_DIR_ENV: &str = "SQZ_LOCALES_DIR";

include!(concat!(env!("OUT_DIR"), "/builtin_locales.rs"));

type Pack = HashMap<String, String>;

/// Language-pack store bound to one selected language.
pub struct Localizer {
    lang: String,
    packs: HashMap<String, Pack>,
}

impl Localizer {
    /// Builds a localizer with the full selection chain:
    /// explicit request (`--lang`) → `SQZ_LANG` → system locale → en-US.
    /// User packs are loaded from `SQZ_LOCALES_DIR` or the platform default
    /// locale directory.
    pub fn load(explicit: Option<&str>) -> Self {
        let requested = explicit
            .map(str::to_owned)
            .or_else(|| std::env::var(LANG_ENV).ok().filter(|s| !s.is_empty()))
            .or_else(sys_locale::get_locale);
        Self::with_user_dir(requested.as_deref(), user_locales_dir().as_deref())
    }

    /// Builds a localizer from an already-resolved language request and an
    /// optional user locale directory (injectable for tests).
    pub fn with_user_dir(requested: Option<&str>, user_dir: Option<&Path>) -> Self {
        let mut packs = builtin_packs();
        if let Some(dir) = user_dir {
            merge_user_packs(&mut packs, dir);
        }
        let lang = negotiate(requested, &packs);
        Self { lang, packs }
    }

    /// The selected language tag (e.g. `"zh-CN"`).
    pub fn language(&self) -> &str {
        &self.lang
    }

    /// All available language tags, sorted (built-in + user packs).
    pub fn available_languages(&self) -> Vec<String> {
        let mut langs: Vec<String> = self.packs.keys().cloned().collect();
        langs.sort();
        langs
    }

    /// All available languages with their self-described display names
    /// (the `meta.name` key of each pack, falling back to the tag itself).
    /// Drives the GUI settings language dropdown.
    pub fn language_names(&self) -> Vec<(String, String)> {
        self.available_languages()
            .into_iter()
            .map(|tag| {
                let name = pack_display_name(&self.packs, &tag);
                (tag, name)
            })
            .collect()
    }

    /// The full key→value table for the selected language with the en-US
    /// fallback already merged in (the GUI fetches this once per language
    /// switch and renders everything frontend-side).
    pub fn table(&self) -> HashMap<String, String> {
        let mut out = fallback_table_or_empty(&self.packs);
        if self.lang != FALLBACK_LANG {
            if let Some(pack) = self.packs.get(&self.lang) {
                out.extend(pack.clone());
            }
        }
        out
    }

    /// Looks up a key without placeholders.
    pub fn t(&self, key: &str) -> String {
        self.format(key, &[])
    }

    /// Looks up a key and substitutes `{name}` placeholders.
    /// Fallback chain: selected language → en-US → the bare key.
    pub fn format(&self, key: &str, args: &[(&str, &str)]) -> String {
        let template = self
            .packs
            .get(&self.lang)
            .and_then(|p| p.get(key))
            .or_else(|| {
                if self.lang != FALLBACK_LANG {
                    log::debug!("i18n: key '{key}' missing in '{}', falling back", self.lang);
                }
                self.packs.get(FALLBACK_LANG).and_then(|p| p.get(key))
            });
        let mut out = match template {
            Some(t) => t.clone(),
            None => {
                log::debug!("i18n: key '{key}' missing in every pack, using the bare key");
                key.to_owned()
            }
        };
        for (name, value) in args {
            out = out.replace(&format!("{{{name}}}"), value);
        }
        out
    }
}

/// Parses the embedded packs. The built-ins are validated by unit tests, so
/// a parse failure here is a programming error caught in CI.
fn builtin_packs() -> HashMap<String, Pack> {
    BUILTIN
        .iter()
        .map(|(lang, json)| {
            let pack = match parse_pack(json) {
                Ok(pack) => pack,
                Err(e) => {
                    log::error!("i18n: built-in pack '{lang}' is invalid: {e}");
                    Pack::new()
                }
            };
            ((*lang).to_owned(), pack)
        })
        .collect()
}

fn pack_display_name(packs: &HashMap<String, Pack>, tag: &str) -> String {
    match packs.get(tag).and_then(|pack| pack.get("meta.name")) {
        Some(name) => name.clone(),
        None => tag.to_owned(),
    }
}

fn fallback_table_or_empty(packs: &HashMap<String, Pack>) -> HashMap<String, String> {
    match packs.get(FALLBACK_LANG) {
        Some(pack) => pack.clone(),
        None => {
            log::error!("i18n: fallback pack '{FALLBACK_LANG}' is unavailable");
            HashMap::new()
        }
    }
}

/// Parses one flat key→value JSON pack. Non-string values are skipped with a
/// debug log entry instead of failing the whole pack.
fn parse_pack(json: &str) -> Result<Pack, serde_json::Error> {
    let map: serde_json::Map<String, serde_json::Value> = serde_json::from_str(json)?;
    let mut pack = Pack::with_capacity(map.len());
    for (key, value) in map {
        match value {
            serde_json::Value::String(s) => {
                pack.insert(key, s);
            }
            other => log::debug!("i18n: ignoring non-string value for key '{key}': {other}"),
        }
    }
    Ok(pack)
}

/// Scans `dir` for `<BCP47>.json` packs and merges them: same-named keys
/// override the built-ins, new file names add new languages. Unreadable or
/// invalid files are skipped (a broken user pack must never break the app).
fn merge_user_packs(packs: &mut HashMap<String, Pack>, dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(lang) = locale_tag_of(&path) else {
            continue;
        };
        let Ok(json) = std::fs::read_to_string(&path) else {
            log::debug!("i18n: cannot read user pack {}", path.display());
            continue;
        };
        match parse_pack(&json) {
            Ok(pack) => {
                packs.entry(lang).or_default().extend(pack);
            }
            Err(e) => log::debug!("i18n: invalid user pack {}: {e}", path.display()),
        }
    }
}

/// Extracts a plausible BCP 47 tag from a `*.json` file name
/// (ASCII alphanumerics and `-` only).
fn locale_tag_of(path: &Path) -> Option<String> {
    if path.extension().and_then(|e| e.to_str()) != Some("json") {
        return None;
    }
    let stem = path.file_stem()?.to_str()?;
    let valid = !stem.is_empty() && stem.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    valid.then(|| stem.to_owned())
}

/// Picks the best available language for a request: exact match
/// (case-insensitive) → same primary language subtag → en-US.
fn negotiate(requested: Option<&str>, packs: &HashMap<String, Pack>) -> String {
    let Some(req) = requested.map(str::trim).filter(|s| !s.is_empty()) else {
        return FALLBACK_LANG.to_owned();
    };
    // Sort for deterministic prefix matching across runs.
    let mut available: Vec<&String> = packs.keys().collect();
    available.sort();
    if let Some(exact) = available.iter().find(|l| l.eq_ignore_ascii_case(req)) {
        return (*exact).clone();
    }
    let primary = primary_subtag_or_self(req);
    if let Some(by_lang) = available.iter().find(|l| {
        l.split('-')
            .next()
            .is_some_and(|p| p.eq_ignore_ascii_case(primary))
    }) {
        return (*by_lang).clone();
    }
    log::debug!("i18n: no pack matches '{req}', falling back to {FALLBACK_LANG}");
    FALLBACK_LANG.to_owned()
}

fn primary_subtag_or_self(tag: &str) -> &str {
    match tag.split('-').next() {
        Some(primary) if !primary.is_empty() => primary,
        _ => tag,
    }
}

/// The user locale directory: `SQZ_LOCALES_DIR` override, else the platform
/// data dir + `Squallz/locales` (macOS: `~/Library/Application Support`).
fn user_locales_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var(LOCALES_DIR_ENV) {
        if !dir.is_empty() {
            return Some(PathBuf::from(dir));
        }
    }
    dirs::data_dir().map(|d| d.join("Squallz").join("locales"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("squallz-i18n-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn builtin_packs_parse_and_cover_same_keys() {
        let packs = builtin_packs();
        let en = packs.get("en-US").unwrap();
        let zh = packs.get("zh-CN").unwrap();
        assert!(!en.is_empty());
        // Both built-ins must define exactly the same key set.
        let mut en_keys: Vec<&String> = en.keys().collect();
        let mut zh_keys: Vec<&String> = zh.keys().collect();
        en_keys.sort();
        zh_keys.sort();
        assert_eq!(en_keys, zh_keys);
    }

    #[test]
    fn builtin_manifest_tracks_locale_files() {
        let locales_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../locales");
        let mut file_tags = Vec::new();
        for entry in std::fs::read_dir(locales_dir).unwrap().flatten() {
            if let Some(tag) = locale_tag_of(&entry.path()) {
                file_tags.push(tag);
            }
        }
        file_tags.sort();

        let mut builtin_tags: Vec<String> =
            BUILTIN.iter().map(|(tag, _)| (*tag).to_owned()).collect();
        builtin_tags.sort();
        assert_eq!(builtin_tags, file_tags);
    }

    #[test]
    fn placeholder_substitution() {
        let loc = Localizer::with_user_dir(Some("zh-CN"), None);
        let msg = loc.format("cli.compress.done", &[("path", "/tmp/a.zip")]);
        assert_eq!(msg, "已创建 /tmp/a.zip");
    }

    #[test]
    fn language_negotiation() {
        assert_eq!(
            Localizer::with_user_dir(Some("zh-CN"), None).language(),
            "zh-CN"
        );
        // Case-insensitive exact match.
        assert_eq!(
            Localizer::with_user_dir(Some("ZH-cn"), None).language(),
            "zh-CN"
        );
        // Primary-subtag match (e.g. a "zh-Hans-CN" system locale).
        assert_eq!(
            Localizer::with_user_dir(Some("zh-Hans-CN"), None).language(),
            "zh-CN"
        );
        // Unknown language falls back to en-US.
        assert_eq!(
            Localizer::with_user_dir(Some("fr-FR"), None).language(),
            "en-US"
        );
        assert_eq!(Localizer::with_user_dir(None, None).language(), "en-US");
    }

    #[test]
    fn table_merges_fallback_and_names_languages() {
        let zh = Localizer::with_user_dir(Some("zh-CN"), None);
        let table = zh.table();
        assert_eq!(table.get("gui.toolbar.compress").unwrap(), "压缩");
        // A full pack covers everything; the fallback merge keeps the size.
        assert!(table.len() >= 200, "expected a sizable merged table");
        let names = zh.language_names();
        assert!(names.contains(&("zh-CN".into(), "简体中文".into())));
        assert!(names.contains(&("en-US".into(), "English".into())));
    }

    #[test]
    fn missing_key_falls_back_to_bare_key() {
        let loc = Localizer::with_user_dir(Some("en-US"), None);
        assert_eq!(loc.t("no.such.key"), "no.such.key");
    }

    #[test]
    fn user_pack_overrides_and_adds_languages() {
        let dir = temp_dir("user-packs");
        // Override one key of a built-in language.
        std::fs::write(
            dir.join("zh-CN.json"),
            r#"{"cli.compress.done": "搞定 {path}"}"#,
        )
        .unwrap();
        // A new BCP 47 file name introduces a new language (partial pack).
        std::fs::write(dir.join("xx-XX.json"), r#"{"error.disk_full": "XX FULL"}"#).unwrap();
        // Invalid files are ignored.
        std::fs::write(dir.join("broken.json"), "{not json").unwrap();
        std::fs::write(dir.join("notes.txt"), "ignored").unwrap();

        let zh = Localizer::with_user_dir(Some("zh-CN"), Some(&dir));
        assert_eq!(zh.format("cli.compress.done", &[("path", "x")]), "搞定 x");
        // Untouched keys keep the built-in text.
        assert_eq!(zh.t("error.disk_full"), "磁盘已满");

        let xx = Localizer::with_user_dir(Some("xx-XX"), Some(&dir));
        assert_eq!(xx.language(), "xx-XX");
        assert_eq!(xx.t("error.disk_full"), "XX FULL");
        // Keys missing from the partial pack fall back to en-US.
        assert_eq!(xx.t("error.wrong_password"), "Wrong password");

        assert!(xx.available_languages().contains(&"xx-XX".to_owned()));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
