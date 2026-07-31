//! Locates the first-party SFX template shipped with the desktop app.

use std::path::{Path, PathBuf};

use squallz_core::SfxTarget;

pub(crate) fn discover_host_template() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?;
    discover_for_executable(SfxTarget::host(), &executable)
}

pub(crate) fn output_extension(target: SfxTarget) -> &'static str {
    match target {
        SfxTarget::Macos => "app",
        SfxTarget::Windows => "exe",
        SfxTarget::Linux => "run",
    }
}

fn discover_for_executable(target: SfxTarget, executable: &Path) -> Option<PathBuf> {
    match target {
        SfxTarget::Macos => discover_macos_template(executable),
        SfxTarget::Windows | SfxTarget::Linux => discover_cli_template(target, executable),
    }
}

fn discover_macos_template(executable: &Path) -> Option<PathBuf> {
    if let Some(bundle) = enclosing_app_bundle(executable) {
        return Some(bundle);
    }
    let parent = executable.parent()?;
    parent
        .ancestors()
        .take(5)
        .map(|root| root.join("bundle/macos/Squallz.app"))
        .find(|candidate| valid_macos_template(candidate))
}

fn enclosing_app_bundle(executable: &Path) -> Option<PathBuf> {
    let macos = executable.parent()?;
    if macos.file_name()?.to_str()? != "MacOS" {
        return None;
    }
    let contents = macos.parent()?;
    if contents.file_name()?.to_str()? != "Contents" {
        return None;
    }
    let bundle = contents.parent()?;
    valid_macos_template(bundle).then(|| bundle.to_path_buf())
}

fn valid_macos_template(bundle: &Path) -> bool {
    bundle
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("app"))
        && bundle.join("Contents/Info.plist").is_file()
        && bundle.join("Contents/MacOS").is_dir()
        && !bundle.join("Contents/Resources/squallz-sfx").exists()
}

fn discover_cli_template(target: SfxTarget, executable: &Path) -> Option<PathBuf> {
    let file_names: &[&str] = match target {
        SfxTarget::Windows => &["sqz.exe", "sqz"],
        SfxTarget::Linux => &["sqz"],
        SfxTarget::Macos => return None,
    };
    let executable_dir = executable.parent()?;
    let mut candidates = Vec::new();
    for file_name in file_names {
        candidates.push(executable_dir.join(file_name));
        candidates.push(executable_dir.join("bin").join(file_name));
        candidates.push(executable_dir.join("resources/bin").join(file_name));
        if let Some(prefix) = executable_dir.parent() {
            candidates.push(prefix.join("lib/Squallz/bin").join(file_name));
            candidates.push(prefix.join("lib/squallz/bin").join(file_name));
        }
    }
    candidates.into_iter().find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "squallz-sfx-runtime-{tag}-{}-{nonce}",
            std::process::id()
        ))
    }

    #[test]
    fn packaged_macos_executable_resolves_its_app_template() {
        let dir = temp_dir("macos-app");
        let bundle = dir.join("Squallz.app");
        let executable = bundle.join("Contents/MacOS/squallz-gui");
        fs::create_dir_all(executable.parent().unwrap()).unwrap();
        fs::write(bundle.join("Contents/Info.plist"), b"plist").unwrap();
        fs::write(&executable, b"binary").unwrap();

        assert_eq!(
            discover_for_executable(SfxTarget::Macos, &executable),
            Some(bundle)
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn installed_linux_executable_resolves_packaged_cli() {
        let dir = temp_dir("linux-cli");
        let executable = dir.join("usr/bin/squallz-gui");
        let cli = dir.join("usr/lib/Squallz/bin/sqz");
        fs::create_dir_all(executable.parent().unwrap()).unwrap();
        fs::create_dir_all(cli.parent().unwrap()).unwrap();
        fs::write(&executable, b"gui").unwrap();
        fs::write(&cli, b"cli").unwrap();

        assert_eq!(
            discover_for_executable(SfxTarget::Linux, &executable),
            Some(cli)
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn windows_resource_without_extension_is_still_a_valid_template() {
        let dir = temp_dir("windows-cli");
        let executable = dir.join("Squallz.exe");
        let cli = dir.join("bin/sqz");
        fs::create_dir_all(cli.parent().unwrap()).unwrap();
        fs::write(&executable, b"gui").unwrap();
        fs::write(&cli, b"cli").unwrap();

        assert_eq!(
            discover_for_executable(SfxTarget::Windows, &executable),
            Some(cli)
        );
        fs::remove_dir_all(dir).unwrap();
    }
}
