//! Locates the first-party SFX template shipped with the desktop app.

use std::fs;
use std::path::{Path, PathBuf};

use squallz_core::{discover_packaged_sfx_runtime, SfxTarget};

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
    let legacy_file_names: &[&str] = match target {
        SfxTarget::Windows => &["sqz.exe", "sqz"],
        SfxTarget::Linux => &["sqz"],
        SfxTarget::Macos => return None,
    };
    let executable_dir = executable.parent()?;
    discover_packaged_sfx_runtime(executable).or_else(|| {
        legacy_file_names
            .iter()
            .map(|file_name| executable_dir.join(file_name))
            .find(|candidate| fs::symlink_metadata(candidate).is_ok())
    })
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
    fn installed_linux_executable_prefers_packaged_sfx_runtime() {
        let dir = temp_dir("linux-runtime");
        let executable = dir.join("usr/bin/squallz-gui");
        let runtime = dir.join("usr/lib/squallz-gui/bin/sqz-sfx.stub");
        let cli = dir.join("usr/lib/Squallz/bin/sqz");
        fs::create_dir_all(executable.parent().unwrap()).unwrap();
        fs::create_dir_all(runtime.parent().unwrap()).unwrap();
        fs::create_dir_all(cli.parent().unwrap()).unwrap();
        fs::write(&executable, b"gui").unwrap();
        fs::write(&runtime, b"runtime").unwrap();
        fs::write(&cli, b"cli").unwrap();

        assert_eq!(
            discover_for_executable(SfxTarget::Linux, &executable),
            Some(runtime)
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn installed_linux_executable_falls_back_to_legacy_cli() {
        let dir = temp_dir("linux-legacy-cli");
        let executable = dir.join("usr/bin/squallz-gui");
        let cli = dir.join("usr/bin/sqz");
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
    fn windows_executable_resolves_packaged_sfx_runtime() {
        let dir = temp_dir("windows-runtime");
        let executable = dir.join("Squallz.exe");
        let runtime = dir.join("bin/sqz-sfx.stub");
        fs::create_dir_all(runtime.parent().unwrap()).unwrap();
        fs::write(&executable, b"gui").unwrap();
        fs::write(&runtime, b"runtime").unwrap();

        assert_eq!(
            discover_for_executable(SfxTarget::Windows, &executable),
            Some(runtime)
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn invalid_dedicated_runtime_is_not_hidden_by_legacy_cli() {
        let dir = temp_dir("invalid-runtime");
        let executable = dir.join("Squallz.exe");
        let runtime = dir.join("bin/sqz-sfx.stub");
        let cli = dir.join("bin/sqz.exe");
        fs::create_dir_all(&runtime).unwrap();
        fs::write(&executable, b"gui").unwrap();
        fs::write(&cli, b"cli").unwrap();

        assert_eq!(
            discover_for_executable(SfxTarget::Windows, &executable),
            Some(runtime)
        );
        fs::remove_dir_all(dir).unwrap();
    }
}
