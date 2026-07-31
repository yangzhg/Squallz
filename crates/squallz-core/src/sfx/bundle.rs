//! macOS SFX app-bundle layout and assembly.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};
use squallz_format_api::{ControlToken, EntryPath, FormatError, ProgressSink, ResourceOptions};

use super::{
    bundle_tree::BundleTree, SfxBuildOptions, SfxBuildReport, SfxInfo, SfxLayout, SfxTarget,
    StagedSfx, COPY_BUFFER_BYTES, SFX_GUI_STUB_MARKER,
};
use crate::filesystem_identity::{open_regular_file_no_follow, RegularFileState};
use crate::Engine;

const MANIFEST_MAGIC: [u8; 8] = *b"SQZSFXB1";
const MANIFEST_VERSION: u32 = 1;
const MANIFEST_LEN: usize = 64;
const RESOURCE_DIR: &str = "squallz-sfx";
const PAYLOAD_NAME: &str = "payload.zip";
const MANIFEST_NAME: &str = "manifest.v1";
const MAX_TEMPLATE_ENTRIES: usize = 200_000;
const MAX_TEMPLATE_DEPTH: usize = 64;
const MAX_INFO_PLIST_BYTES: u64 = 1024 * 1024;
// A bundle is a filesystem tree, so logical file lengths alone are not a
// safe free-space guard. Every emitted node reserves rounded content, its
// encoded path, and separate node/parent-directory metadata allocations.
const BUNDLE_BASE_SLACK_BYTES: u64 = 1024 * 1024;
const MIN_ALLOCATION_GRANULARITY: u64 = 4096;
const ENTRY_METADATA_ALLOCATIONS: u64 = 2;
const DESKTOP_QUICK_LOOK_EXTENSION: &str = "Contents/PlugIns/SquallzQuickLook.appex";

#[derive(Debug)]
enum TemplateEntryKind {
    Directory,
    File { state: RegularFileState },
    Symlink { target: PathBuf },
}

#[derive(Debug)]
struct TemplateEntry {
    relative: PathBuf,
    kind: TemplateEntryKind,
    identity: super::transaction::PathIdentity,
    permissions: fs::Permissions,
}

#[derive(Debug)]
pub(super) struct PreparedTemplate {
    template: PathBuf,
    root_identity: super::transaction::PathIdentity,
    root_permissions: fs::Permissions,
    executable_relative: PathBuf,
    minimum_system_version: String,
    entries: Vec<TemplateEntry>,
}

impl PreparedTemplate {
    pub(super) fn output_budget(
        &self,
        dest: &Path,
        payload_bytes: u64,
    ) -> Result<u64, FormatError> {
        let executable = self.template.join(&self.executable_relative);
        let metadata =
            render_bundle_metadata(dest, &executable, &self.minimum_system_version, [0u8; 32])?;
        bundle_output_budget(dest, &self.entries, payload_bytes, &metadata)
    }

    fn executable_entry(&self) -> Result<&TemplateEntry, FormatError> {
        self.entries
            .iter()
            .find(|entry| entry.relative == self.executable_relative)
            .filter(|entry| matches!(entry.kind, TemplateEntryKind::File { .. }))
            .ok_or_else(|| {
                FormatError::CorruptArchive(
                    "app template executable is missing from its prepared manifest".into(),
                )
            })
    }

    fn validate_root(&self) -> Result<(), FormatError> {
        let metadata = prepared_path_metadata(&self.template)?;
        if !metadata.is_dir()
            || metadata.file_type().is_symlink()
            || prepared_path_identity(&self.template)? != self.root_identity
        {
            return Err(template_changed(&self.template));
        }
        Ok(())
    }

    pub(super) fn validate_runtime(
        &self,
        resources: &ResourceOptions,
        ctl: &ControlToken,
    ) -> Result<(), FormatError> {
        self.validate_root()?;
        let executable_entry = self.executable_entry()?;
        let executable = self.template.join(&self.executable_relative);
        let mut executable_file = open_template_file(&executable, executable_entry)?;
        let target = super::executable_target_from_file(&mut executable_file, &executable)?;
        let mut marker_file = open_template_file(&executable, executable_entry)?;
        if target != SfxTarget::Macos
            || !super::file_has_marker(&mut marker_file, &SFX_GUI_STUB_MARKER, resources, ctl)?
        {
            return Err(FormatError::Unsupported(
                "macOS SFX template must contain a Squallz GUI SFX-capable Mach-O executable"
                    .into(),
            ));
        }
        Ok(())
    }
}

pub(super) fn prepare_template(template: &Path) -> Result<PreparedTemplate, FormatError> {
    let root_metadata = fs::symlink_metadata(template)?;
    if !root_metadata.is_dir() || root_metadata.file_type().is_symlink() || !is_app_path(template) {
        return Err(FormatError::Unsupported(
            "macOS SFX template must be a non-symlink .app bundle".into(),
        ));
    }
    let root_identity = super::transaction::path_identity(template)?;
    validate_template_output_layout(template)?;
    let plist = template_info_plist(template)?;
    let executable = template_executable(template, &plist)?;
    let executable_relative = executable
        .strip_prefix(template)
        .map(Path::to_path_buf)
        .map_err(|_| {
            FormatError::Unsupported("app template executable is outside the bundle".into())
        })?;
    let minimum_system_version = template_minimum_system_version(&plist)?;
    let entries = scan_template(template)?;
    let prepared = PreparedTemplate {
        template: template.to_path_buf(),
        root_identity,
        root_permissions: root_metadata.permissions(),
        executable_relative,
        minimum_system_version,
        entries,
    };
    prepared.executable_entry()?;
    prepared.validate_root()?;
    Ok(prepared)
}

#[derive(Debug)]
struct BundleMetadata {
    plist: String,
    localized: String,
}

#[derive(Debug, Clone, Copy)]
struct BundleManifest {
    payload_bytes: u64,
    payload_sha256: [u8; 32],
}

impl BundleManifest {
    fn encode(self) -> [u8; MANIFEST_LEN] {
        let mut bytes = [0u8; MANIFEST_LEN];
        bytes[..8].copy_from_slice(&MANIFEST_MAGIC);
        bytes[8..12].copy_from_slice(&MANIFEST_VERSION.to_le_bytes());
        bytes[16..24].copy_from_slice(&self.payload_bytes.to_le_bytes());
        bytes[24..56].copy_from_slice(&self.payload_sha256);
        bytes
    }

    fn decode(bytes: &[u8; MANIFEST_LEN]) -> Result<Self, FormatError> {
        if bytes[..8] != MANIFEST_MAGIC {
            return Err(FormatError::CorruptArchive(
                "invalid macOS SFX bundle manifest magic".into(),
            ));
        }
        let version = u32::from_le_bytes(copy_array(&bytes[8..12])?);
        if version != MANIFEST_VERSION {
            return Err(FormatError::CorruptArchive(format!(
                "unsupported macOS SFX bundle manifest version {version}"
            )));
        }
        if bytes[12..16].iter().any(|byte| *byte != 0) || bytes[56..].iter().any(|byte| *byte != 0)
        {
            return Err(FormatError::CorruptArchive(
                "unsupported macOS SFX bundle manifest flags".into(),
            ));
        }
        let payload_bytes = u64::from_le_bytes(copy_array(&bytes[16..24])?);
        if payload_bytes == 0 {
            return Err(FormatError::CorruptArchive(
                "macOS SFX bundle payload is empty".into(),
            ));
        }
        let payload_sha256 = copy_array(&bytes[24..56])?;
        Ok(Self {
            payload_bytes,
            payload_sha256,
        })
    }
}

pub(super) fn stage(
    engine: &Engine,
    prepared: PreparedTemplate,
    mut archive: super::BoundSfxPayload,
    dest: &Path,
    opts: &SfxBuildOptions,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
) -> Result<StagedSfx, FormatError> {
    validate_paths(&prepared.template, archive.path(), dest, opts.overwrite)?;
    prepared.validate_runtime(&opts.resources, ctl)?;
    archive.verify()?;
    let executable = prepared.template.join(&prepared.executable_relative);

    let template_bytes = template_file_bytes(&prepared.entries)?;
    let payload_bytes = archive.len();
    let metadata_budget = render_bundle_metadata(
        dest,
        &executable,
        &prepared.minimum_system_version,
        [0u8; 32],
    )?;
    let required = bundle_output_budget(dest, &prepared.entries, payload_bytes, &metadata_budget)?;
    super::ensure_destination_space(dest, required)?;

    let (tmp, staged_identity) =
        super::transaction::reserve_staged_path(dest, SfxLayout::MacosApp)?;
    let held_root = open_bundle_root(&tmp)?;
    if super::transaction::file_identity(&held_root)? != staged_identity
        || super::transaction::path_identity(&tmp)? != staged_identity
    {
        return Err(FormatError::Io(io::Error::other(
            "SFX bundle staging changed while it was opened",
        )));
    }
    let tree = BundleTree::new(&held_root, &tmp)?;
    let result = (|| {
        let mut overall_done = 0u64;
        let overall_total = template_bytes
            .checked_add(payload_bytes)
            .ok_or_else(|| FormatError::ResourceLimitExceeded("SFX bundle size overflow".into()))?;
        copy_template(
            &prepared,
            &tree,
            &opts.resources,
            progress,
            ctl,
            &mut overall_done,
            overall_total,
        )?;

        tree.ensure_dir(Path::new("Contents/Resources"))?;
        if !prepared.entries.iter().any(|entry| {
            entry.relative == Path::new("Contents/Resources")
                && matches!(entry.kind, TemplateEntryKind::Directory)
        }) {
            set_generated_directory_permissions(&tree, Path::new("Contents/Resources"))?;
        }
        let private_resources = resource_relative_dir();
        tree.create_dir(&private_resources)?;
        set_generated_directory_permissions(&tree, &private_resources)?;
        let payload_relative = private_resources.join(PAYLOAD_NAME);
        let payload_dest = tmp.join(&payload_relative);
        let mut payload_output = tree.create_file(&payload_relative)?;
        let payload_sha256 = copy_payload(
            &mut archive,
            &mut payload_output,
            &opts.resources,
            progress,
            ctl,
            &mut overall_done,
            overall_total,
            payload_bytes,
        )?;
        set_generated_file_permissions(&payload_output)?;
        payload_output.sync_all()?;
        let mut manifest_output = tree.create_file(&private_resources.join(MANIFEST_NAME))?;
        manifest_output.write_all(
            &BundleManifest {
                payload_bytes,
                payload_sha256,
            }
            .encode(),
        )?;
        set_generated_file_permissions(&manifest_output)?;
        manifest_output.sync_all()?;
        let metadata = render_bundle_metadata(
            dest,
            &executable,
            &prepared.minimum_system_version,
            payload_sha256,
        )?;
        if bundle_output_budget(dest, &prepared.entries, payload_bytes, &metadata)? > required {
            return Err(FormatError::ResourceLimitExceeded(
                "generated macOS SFX metadata exceeded its planned budget".into(),
            ));
        }
        write_bundle_metadata(&tree, &prepared, &metadata)?;
        apply_template_directory_permissions(&prepared, &tree)?;
        sync_bundle_directories(&prepared, &tree)?;

        ensure_staged_root_binding(
            &held_root,
            &tmp,
            staged_identity,
            "SFX bundle staging changed during assembly",
        )?;
        let info = inspect_contents(&tmp)?.ok_or_else(|| {
            FormatError::CorruptArchive("assembled macOS SFX bundle is not readable".into())
        })?;
        verify(
            &tmp,
            info,
            &opts.resources,
            &squallz_format_api::NoProgress,
            ctl,
        )?;
        let reader = engine.open(&payload_dest, &squallz_format_api::OpenOptions::default())?;
        drop(reader);
        let total_bytes = directory_bytes(&tmp)?;
        ensure_staged_root_binding(
            &held_root,
            &tmp,
            staged_identity,
            "SFX bundle staging changed during assembly",
        )?;
        Ok(StagedSfx {
            path: tmp.clone(),
            identity: staged_identity,
            held_file: Some(held_root),
            progress_total: overall_total,
            report: SfxBuildReport {
                path: dest.to_path_buf(),
                target: SfxTarget::Macos,
                layout: SfxLayout::MacosApp,
                stub_bytes: template_bytes,
                payload_bytes,
                total_bytes,
                payload_crc32: 0,
                payload_sha256: Some(payload_sha256),
                requires_signing: true,
                preserved_outputs: Vec::new(),
            },
        })
    })();
    match result {
        Ok(staged) => Ok(staged),
        Err(error) => Err(super::transaction::merge_cleanup_result(
            error,
            super::transaction::discard_staged_path(
                &tmp,
                staged_identity,
                SfxLayout::MacosApp,
                dest,
            ),
            dest,
        )),
    }
}

fn ensure_staged_root_binding(
    held_root: &File,
    path: &Path,
    identity: super::transaction::PathIdentity,
    message: &str,
) -> Result<(), FormatError> {
    if super::transaction::file_identity(held_root)? != identity
        || super::transaction::path_identity(path)? != identity
    {
        return Err(FormatError::Io(io::Error::other(message)));
    }
    Ok(())
}

#[cfg(not(windows))]
fn open_bundle_root(path: &Path) -> io::Result<File> {
    File::open(path)
}

#[cfg(windows)]
fn open_bundle_root(path: &Path) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE,
        FILE_SHARE_READ,
    };

    fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}

pub(super) fn inspect(path: &Path) -> Result<Option<SfxInfo>, FormatError> {
    if !is_app_path(path) {
        return Ok(None);
    }
    inspect_contents(path)
}

fn inspect_contents(path: &Path) -> Result<Option<SfxInfo>, FormatError> {
    let manifest_path = manifest_path(path);
    let payload_path = payload_path(path);
    if fs::symlink_metadata(&manifest_path).is_err() && fs::symlink_metadata(&payload_path).is_err()
    {
        return Ok(None);
    }
    require_regular_file(&manifest_path, "macOS SFX manifest")?;
    require_regular_file(&payload_path, "macOS SFX payload")?;
    let manifest = read_manifest(&manifest_path)?;
    let actual_payload_bytes = fs::metadata(&payload_path)?.len();
    if actual_payload_bytes != manifest.payload_bytes {
        return Err(FormatError::CorruptArchive(
            "macOS SFX payload length does not match its manifest".into(),
        ));
    }
    let total_bytes = directory_bytes(path)?;
    let stub_bytes_value = total_bytes
        .saturating_sub(actual_payload_bytes)
        .saturating_sub(MANIFEST_LEN as u64);
    Ok(Some(SfxInfo {
        layout: SfxLayout::MacosApp,
        target: SfxTarget::Macos,
        payload_offset: 0,
        payload_bytes: actual_payload_bytes,
        payload_crc32: 0,
        payload_sha256: Some(manifest.payload_sha256),
        total_bytes,
        stub_bytes_value,
    }))
}

pub(super) fn verify(
    path: &Path,
    info: SfxInfo,
    resources: &ResourceOptions,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
) -> Result<SfxInfo, FormatError> {
    let expected = info.payload_sha256.ok_or_else(|| {
        FormatError::CorruptArchive("macOS SFX manifest has no SHA-256 digest".into())
    })?;
    let mut file = open_payload(path)?;
    let mut buffer = vec![0u8; resources.stream_buffer_size(COPY_BUFFER_BYTES)?];
    let mut hasher = Sha256::new();
    let mut done = 0u64;
    let entry = EntryPath::from_utf8(PAYLOAD_NAME);
    loop {
        ctl.checkpoint()?;
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        done = done.saturating_add(read as u64);
        progress.on_entry_progress(done, info.payload_bytes, &entry, done, info.payload_bytes);
    }
    if done != info.payload_bytes {
        return Err(FormatError::CorruptArchive(
            "macOS SFX payload ended before its declared length".into(),
        ));
    }
    let actual: [u8; 32] = hasher.finalize().into();
    if actual != expected {
        return Err(FormatError::CorruptArchive(
            "macOS SFX payload SHA-256 mismatch".into(),
        ));
    }
    Ok(info)
}

pub(super) fn open_payload(path: &Path) -> Result<File, FormatError> {
    let payload = payload_path(path);
    require_regular_file(&payload, "macOS SFX payload")?;
    Ok(File::open(payload)?)
}

pub(super) fn for_executable(executable: &Path) -> Option<PathBuf> {
    let macos = executable.parent()?;
    if macos.file_name()? != OsStr::new("MacOS") {
        return None;
    }
    let contents = macos.parent()?;
    if contents.file_name()? != OsStr::new("Contents") {
        return None;
    }
    let bundle = contents.parent()?;
    if !is_app_path(bundle) || fs::symlink_metadata(manifest_path(bundle)).is_err() {
        return None;
    }
    Some(bundle.to_path_buf())
}

fn validate_paths(
    template: &Path,
    archive: &Path,
    dest: &Path,
    overwrite: bool,
) -> Result<(), FormatError> {
    let template_metadata = fs::symlink_metadata(template)?;
    if !template_metadata.is_dir()
        || template_metadata.file_type().is_symlink()
        || !is_app_path(template)
    {
        return Err(FormatError::Unsupported(
            "macOS SFX template must be a non-symlink .app bundle".into(),
        ));
    }
    if !fs::metadata(archive)?.is_file() {
        return Err(FormatError::Unsupported(
            "SFX payload must be a regular archive file".into(),
        ));
    }
    if !is_app_path(dest) {
        return Err(FormatError::Unsupported(
            "macOS SFX output must use the .app extension".into(),
        ));
    }
    if crate::same_existing_path(template, archive)
        || crate::same_existing_path(template, dest)
        || crate::same_existing_path(archive, dest)
        || destination_is_inside_template(template, dest)?
    {
        return Err(FormatError::Unsupported(
            "SFX template, payload, and output paths must be separate".into(),
        ));
    }
    super::validate_publish_destination(dest, SfxLayout::MacosApp, overwrite)?;
    validate_template_output_layout(template)?;
    if super::inspect_sfx(archive)?.is_some() {
        return Err(FormatError::Unsupported(
            "an SFX artifact cannot be nested as the payload of another SFX artifact".into(),
        ));
    }
    Ok(())
}

fn validate_template_output_layout(template: &Path) -> Result<(), FormatError> {
    require_template_directory(&template.join("Contents"), "app template Contents")?;
    require_template_directory(
        &template.join("Contents/MacOS"),
        "app template Contents/MacOS",
    )?;
    match fs::symlink_metadata(resource_dir(template)) {
        Ok(_) => {
            return Err(FormatError::Unsupported(
                "an existing macOS SFX bundle cannot be reused as a template".into(),
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let template_resources = template.join("Contents/Resources");
    match fs::symlink_metadata(&template_resources) {
        Ok(metadata) if !metadata.is_dir() || metadata.file_type().is_symlink() => {
            return Err(FormatError::Unsupported(
                "app template Contents/Resources must be a non-symlink directory".into(),
            ));
        }
        Ok(_) => {
            for locale in ["en.lproj", "zh-Hans.lproj"] {
                let locale = template_resources.join(locale);
                match fs::symlink_metadata(&locale) {
                    Ok(metadata) if !metadata.is_dir() || metadata.file_type().is_symlink() => {
                        return Err(FormatError::Unsupported(format!(
                            "app template {} must be a non-symlink directory",
                            locale.display()
                        )));
                    }
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

fn require_template_directory(path: &Path, label: &str) -> Result<(), FormatError> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(FormatError::Unsupported(format!(
            "{label} must be a non-symlink directory"
        )));
    }
    Ok(())
}

fn template_info_plist(template: &Path) -> Result<String, FormatError> {
    let plist_path = template.join("Contents/Info.plist");
    let path_metadata = fs::symlink_metadata(&plist_path)?;
    if !path_metadata.is_file() || path_metadata.file_type().is_symlink() {
        return Err(FormatError::CorruptArchive(
            "app template Info.plist must be a regular file".into(),
        ));
    }
    let identity = super::transaction::path_identity(&plist_path)?;
    let mut file = open_regular_file_no_follow(&plist_path)?;
    let state = RegularFileState::from_metadata(&file.metadata()?);
    if super::transaction::file_identity(&file)? != identity
        || super::transaction::path_identity(&plist_path)? != identity
    {
        return Err(template_changed(&plist_path));
    }
    if state.bytes() > MAX_INFO_PLIST_BYTES {
        return Err(FormatError::ResourceLimitExceeded(
            "app template Info.plist exceeds 1 MiB".into(),
        ));
    }
    let mut plist = String::new();
    (&mut file)
        .take(MAX_INFO_PLIST_BYTES + 1)
        .read_to_string(&mut plist)
        .map_err(|_| FormatError::Unsupported("app template must use an XML Info.plist".into()))?;
    if plist.len() as u64 > MAX_INFO_PLIST_BYTES {
        return Err(FormatError::ResourceLimitExceeded(
            "app template Info.plist exceeds 1 MiB".into(),
        ));
    }
    if plist.len() as u64 != state.bytes()
        || !state.matches(&file.metadata()?)
        || super::transaction::path_identity(&plist_path)? != identity
    {
        return Err(template_changed(&plist_path));
    }
    Ok(plist)
}

fn plist_string_value<'a>(plist: &'a str, key: &str, field: &str) -> Result<&'a str, FormatError> {
    let key_tag = format!("<key>{key}</key>");
    let tail = plist
        .split_once(&key_tag)
        .map(|(_, tail)| tail)
        .ok_or_else(|| {
            FormatError::Unsupported(format!("app template Info.plist has no {field}"))
        })?;
    let value = tail
        .trim_start()
        .strip_prefix("<string>")
        .ok_or_else(|| FormatError::Unsupported(format!("app template {field} is not a string")))?;
    let value = value
        .split_once("</string>")
        .map(|(value, _)| value)
        .ok_or_else(|| FormatError::Unsupported(format!("app template {field} is truncated")))?;
    let value = value.trim();
    if value.is_empty() {
        return Err(FormatError::Unsupported(format!(
            "app template {field} is empty"
        )));
    }
    Ok(value)
}

fn template_executable(template: &Path, plist: &str) -> Result<PathBuf, FormatError> {
    let name = plist_string_value(plist, "CFBundleExecutable", "CFBundleExecutable")?;
    if name.is_empty() || name.contains('/') || name.contains('\\') {
        return Err(FormatError::Unsupported(
            "app template CFBundleExecutable is invalid".into(),
        ));
    }
    let executable = template.join("Contents/MacOS").join(name);
    require_regular_file(&executable, "app template executable")?;
    Ok(executable)
}

fn template_minimum_system_version(plist: &str) -> Result<String, FormatError> {
    plist_string_value(plist, "LSMinimumSystemVersion", "LSMinimumSystemVersion").map(str::to_owned)
}

fn scan_template(template: &Path) -> Result<Vec<TemplateEntry>, FormatError> {
    let mut entries = Vec::new();
    scan_dir(template, Path::new(""), 0, &mut entries)?;
    Ok(entries)
}

fn is_desktop_quick_look_extension(relative: &Path) -> bool {
    relative.starts_with(Path::new(DESKTOP_QUICK_LOOK_EXTENSION))
}

fn scan_dir(
    template: &Path,
    relative: &Path,
    depth: usize,
    entries: &mut Vec<TemplateEntry>,
) -> Result<(), FormatError> {
    if depth > MAX_TEMPLATE_DEPTH {
        return Err(FormatError::ResourceLimitExceeded(
            "app template directory depth exceeds 64".into(),
        ));
    }
    for item in fs::read_dir(template.join(relative))? {
        let item = item?;
        let child = relative.join(item.file_name());
        if child == Path::new("Contents/Info.plist")
            || child.starts_with(Path::new("Contents/_CodeSignature"))
            || child.starts_with(resource_relative_dir())
            || is_localized_info_plist(&child)
            || is_desktop_quick_look_extension(&child)
        {
            continue;
        }
        if entries.len() >= MAX_TEMPLATE_ENTRIES {
            return Err(FormatError::ResourceLimitExceeded(
                "app template contains too many entries".into(),
            ));
        }
        let metadata = fs::symlink_metadata(item.path())?;
        let identity = super::transaction::path_identity(&item.path())?;
        let kind = if metadata.file_type().is_symlink() {
            let target = fs::read_link(item.path())?;
            validate_template_symlink_support(&child)?;
            validate_symlink(template, &child, &target)?;
            TemplateEntryKind::Symlink { target }
        } else if metadata.is_dir() {
            TemplateEntryKind::Directory
        } else if metadata.is_file() {
            TemplateEntryKind::File {
                state: RegularFileState::from_metadata(&metadata),
            }
        } else {
            return Err(FormatError::Unsupported(format!(
                "unsupported app template entry: {}",
                child.display()
            )));
        };
        if super::transaction::path_identity(&item.path())? != identity {
            return Err(template_changed(&item.path()));
        }
        let is_dir = matches!(kind, TemplateEntryKind::Directory);
        entries.push(TemplateEntry {
            relative: child.clone(),
            kind,
            identity,
            permissions: metadata.permissions(),
        });
        if is_dir {
            validate_scanned_directory(&item.path(), identity)?;
            scan_dir(template, &child, depth + 1, entries)?;
            validate_scanned_directory(&item.path(), identity)?;
        }
    }
    Ok(())
}

fn validate_scanned_directory(
    path: &Path,
    identity: super::transaction::PathIdentity,
) -> Result<(), FormatError> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || super::transaction::path_identity(path)? != identity
    {
        return Err(template_changed(path));
    }
    Ok(())
}

#[cfg(unix)]
fn validate_template_symlink_support(_relative: &Path) -> Result<(), FormatError> {
    Ok(())
}

#[cfg(not(unix))]
fn validate_template_symlink_support(relative: &Path) -> Result<(), FormatError> {
    Err(FormatError::Unsupported(format!(
        "macOS app template symlinks require a Unix host: {}",
        relative.display()
    )))
}

fn template_file_bytes(entries: &[TemplateEntry]) -> Result<u64, FormatError> {
    entries
        .iter()
        .try_fold(0u64, |total, entry| match entry.kind {
            TemplateEntryKind::File { ref state } => {
                total.checked_add(state.bytes()).ok_or_else(|| {
                    FormatError::ResourceLimitExceeded("app template size overflow".into())
                })
            }
            _ => Ok(total),
        })
}

fn bundle_output_budget(
    dest: &Path,
    entries: &[TemplateEntry],
    payload_bytes: u64,
    metadata: &BundleMetadata,
) -> Result<u64, FormatError> {
    let generated = generated_bundle_entries(entries, payload_bytes, metadata)?;
    validate_bundle_entry_count(entries.len(), generated.len())?;
    let allocation = destination_allocation_granularity(dest)?;
    let root = dest
        .file_name()
        .map(Path::new)
        .unwrap_or_else(|| Path::new(""));
    let mut total = round_up_allocation(BUNDLE_BASE_SLACK_BYTES, allocation)?;
    total = checked_budget_add(total, entry_output_budget(root, 0, allocation)?)?;
    for entry in entries {
        let stored_bytes = match &entry.kind {
            TemplateEntryKind::Directory => 0,
            TemplateEntryKind::File { state } => state.bytes(),
            TemplateEntryKind::Symlink { target } => encoded_path_bytes(target)?,
        };
        total = checked_budget_add(
            total,
            entry_output_budget(&entry.relative, stored_bytes, allocation)?,
        )?;
    }
    for (relative, stored_bytes) in generated {
        total = checked_budget_add(
            total,
            entry_output_budget(&relative, stored_bytes, allocation)?,
        )?;
    }
    Ok(total)
}

fn generated_bundle_entries(
    entries: &[TemplateEntry],
    payload_bytes: u64,
    metadata: &BundleMetadata,
) -> Result<Vec<(PathBuf, u64)>, FormatError> {
    let resources = Path::new("Contents/Resources");
    let mut generated = Vec::with_capacity(7);
    generated.push((
        PathBuf::from("Contents/Info.plist"),
        usize_to_budget_bytes(metadata.plist.len())?,
    ));
    if !entries.iter().any(|entry| entry.relative == resources) {
        generated.push((resources.to_path_buf(), 0));
    }
    let private_resources = resource_relative_dir();
    generated.push((private_resources.clone(), 0));
    generated.push((private_resources.join(PAYLOAD_NAME), payload_bytes));
    generated.push((private_resources.join(MANIFEST_NAME), MANIFEST_LEN as u64));
    for locale in ["en.lproj", "zh-Hans.lproj"] {
        let locale = resources.join(locale);
        if entries.iter().any(|entry| entry.relative == locale) {
            generated.push((
                locale.join("InfoPlist.strings"),
                usize_to_budget_bytes(metadata.localized.len())?,
            ));
        }
    }
    Ok(generated)
}

fn validate_bundle_entry_count(
    template_entries: usize,
    generated_entries: usize,
) -> Result<(), FormatError> {
    let output_entries = template_entries
        .checked_add(generated_entries)
        .ok_or_else(|| FormatError::ResourceLimitExceeded("SFX bundle entry overflow".into()))?;
    if output_entries > MAX_TEMPLATE_ENTRIES {
        return Err(FormatError::ResourceLimitExceeded(
            "app template leaves no room for generated SFX bundle entries".into(),
        ));
    }
    Ok(())
}

fn destination_allocation_granularity(dest: &Path) -> Result<u64, FormatError> {
    let parent = dest
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    Ok(fs4::allocation_granularity(parent)?.max(MIN_ALLOCATION_GRANULARITY))
}

fn entry_output_budget(
    relative: &Path,
    stored_bytes: u64,
    allocation: u64,
) -> Result<u64, FormatError> {
    let content = round_up_allocation(stored_bytes, allocation)?;
    let path = round_up_allocation(encoded_path_bytes(relative)?, allocation)?;
    let metadata = allocation
        .checked_mul(ENTRY_METADATA_ALLOCATIONS)
        .ok_or_else(bundle_budget_overflow)?;
    checked_budget_add(checked_budget_add(content, path)?, metadata)
}

fn encoded_path_bytes(path: &Path) -> Result<u64, FormatError> {
    usize_to_budget_bytes(path.as_os_str().as_encoded_bytes().len())
}

fn usize_to_budget_bytes(value: usize) -> Result<u64, FormatError> {
    u64::try_from(value).map_err(|_| bundle_budget_overflow())
}

fn round_up_allocation(bytes: u64, allocation: u64) -> Result<u64, FormatError> {
    if bytes == 0 {
        return Ok(0);
    }
    bytes
        .checked_add(allocation - 1)
        .map(|value| value / allocation * allocation)
        .ok_or_else(bundle_budget_overflow)
}

fn checked_budget_add(left: u64, right: u64) -> Result<u64, FormatError> {
    left.checked_add(right).ok_or_else(bundle_budget_overflow)
}

fn bundle_budget_overflow() -> FormatError {
    FormatError::ResourceLimitExceeded("SFX bundle size overflow".into())
}

fn validate_template_directory(path: &Path, entry: &TemplateEntry) -> Result<(), FormatError> {
    let metadata = prepared_path_metadata(path)?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || prepared_path_identity(path)? != entry.identity
    {
        return Err(template_changed(path));
    }
    Ok(())
}

fn validate_template_symlink(
    path: &Path,
    entry: &TemplateEntry,
    target: &Path,
) -> Result<(), FormatError> {
    let metadata = prepared_path_metadata(path)?;
    if !metadata.file_type().is_symlink()
        || prepared_path_identity(path)? != entry.identity
        || fs::read_link(path)? != target
        || prepared_path_identity(path)? != entry.identity
    {
        return Err(template_changed(path));
    }
    Ok(())
}

fn validate_template_file_path(path: &Path, entry: &TemplateEntry) -> Result<(), FormatError> {
    let TemplateEntryKind::File { state } = &entry.kind else {
        return Err(FormatError::Other(
            "prepared macOS SFX entry is not a regular file".into(),
        ));
    };
    let identity_before = prepared_path_identity(path)?;
    let metadata = prepared_path_metadata(path)?;
    if identity_before != entry.identity
        || !metadata.is_file()
        || metadata.file_type().is_symlink()
        || !state.matches(&metadata)
        || prepared_path_identity(path)? != entry.identity
    {
        return Err(template_changed(path));
    }
    Ok(())
}

fn open_template_file(path: &Path, entry: &TemplateEntry) -> Result<File, FormatError> {
    validate_template_file_path(path, entry)?;
    let file = open_regular_file_no_follow(path)?;
    validate_template_file_handle(&file, path, entry)?;
    validate_template_file_path(path, entry)?;
    Ok(file)
}

fn validate_template_file_handle(
    file: &File,
    path: &Path,
    entry: &TemplateEntry,
) -> Result<(), FormatError> {
    let TemplateEntryKind::File { state } = &entry.kind else {
        return Err(FormatError::Other(
            "prepared macOS SFX entry is not a regular file".into(),
        ));
    };
    if super::transaction::file_identity(file)? != entry.identity
        || !state.matches(&file.metadata()?)
    {
        return Err(template_changed(path));
    }
    Ok(())
}

fn template_changed(path: &Path) -> FormatError {
    FormatError::CorruptArchive(format!(
        "app template changed while assembling the SFX bundle: {}",
        path.display()
    ))
}

fn prepared_path_metadata(path: &Path) -> Result<fs::Metadata, FormatError> {
    fs::symlink_metadata(path).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            template_changed(path)
        } else {
            error.into()
        }
    })
}

fn prepared_path_identity(path: &Path) -> Result<super::transaction::PathIdentity, FormatError> {
    super::transaction::path_identity(path).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            template_changed(path)
        } else {
            error.into()
        }
    })
}

#[allow(clippy::too_many_arguments)]
fn copy_template(
    prepared: &PreparedTemplate,
    tree: &BundleTree,
    resources: &ResourceOptions,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
    overall_done: &mut u64,
    overall_total: u64,
) -> Result<(), FormatError> {
    prepared.validate_root()?;
    for entry in &prepared.entries {
        ctl.checkpoint()?;
        let source = prepared.template.join(&entry.relative);
        match &entry.kind {
            TemplateEntryKind::Directory => {
                validate_template_directory(&source, entry)?;
                tree.create_dir(&entry.relative)?;
            }
            TemplateEntryKind::File { state } => {
                let mut input = open_template_file(&source, entry)?;
                let mut output = tree.create_file(&entry.relative)?;
                copy_file_from(
                    &mut input,
                    &source,
                    &mut output,
                    state.bytes(),
                    resources,
                    progress,
                    ctl,
                    overall_done,
                    overall_total,
                    &entry.relative,
                    None,
                )?;
                validate_template_file_handle(&input, &source, entry)?;
                validate_template_file_path(&source, entry)?;
                output.set_permissions(entry.permissions.clone())?;
                output.sync_all()?;
            }
            TemplateEntryKind::Symlink { target } => {
                validate_template_symlink(&source, entry, target)?;
                tree.create_symlink(target, &entry.relative)?;
            }
        }
    }
    Ok(())
}

fn apply_template_directory_permissions(
    prepared: &PreparedTemplate,
    tree: &BundleTree,
) -> Result<(), FormatError> {
    for entry in prepared.entries.iter().rev() {
        if matches!(entry.kind, TemplateEntryKind::Directory) {
            validate_template_directory(&prepared.template.join(&entry.relative), entry)?;
            tree.set_permissions(&entry.relative, entry.permissions.clone(), true)?;
        }
    }
    prepared.validate_root()?;
    tree.set_permissions(Path::new(""), prepared.root_permissions.clone(), true)?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn copy_payload(
    source: &mut super::BoundSfxPayload,
    output: &mut File,
    resources: &ResourceOptions,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
    overall_done: &mut u64,
    overall_total: u64,
    expected: u64,
) -> Result<[u8; 32], FormatError> {
    let mut hasher = Sha256::new();
    source.verify()?;
    source.file_mut().seek(SeekFrom::Start(0))?;
    let source_path = source.path().to_path_buf();
    copy_file_from(
        source.file_mut(),
        &source_path,
        output,
        expected,
        resources,
        progress,
        ctl,
        overall_done,
        overall_total,
        Path::new(PAYLOAD_NAME),
        Some(&mut hasher),
    )?;
    source.verify()?;
    Ok(hasher.finalize().into())
}

#[allow(clippy::too_many_arguments)]
fn copy_file_from(
    input: &mut File,
    source: &Path,
    output: &mut File,
    expected: u64,
    resources: &ResourceOptions,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
    overall_done: &mut u64,
    overall_total: u64,
    label: &Path,
    mut hasher: Option<&mut Sha256>,
) -> Result<(), FormatError> {
    let mut buffer = vec![0u8; resources.stream_buffer_size(COPY_BUFFER_BYTES)?];
    let mut current = 0u64;
    let entry = EntryPath::from_utf8(
        label
            .to_string_lossy()
            .replace(std::path::MAIN_SEPARATOR, "/"),
    );
    while current < expected {
        ctl.checkpoint()?;
        let limit = (expected - current).min(buffer.len() as u64) as usize;
        let read = input.read(&mut buffer[..limit])?;
        if read == 0 {
            return Err(FormatError::CorruptArchive(format!(
                "{} changed while assembling the SFX bundle",
                source.display()
            )));
        }
        output.write_all(&buffer[..read])?;
        if let Some(hasher) = hasher.as_mut() {
            hasher.update(&buffer[..read]);
        }
        current += read as u64;
        *overall_done += read as u64;
        progress.on_entry_progress(*overall_done, overall_total, &entry, current, expected);
    }
    let mut extra = [0u8; 1];
    if input.read(&mut extra)? != 0 {
        return Err(FormatError::CorruptArchive(format!(
            "{} changed while assembling the SFX bundle",
            source.display()
        )));
    }
    Ok(())
}

fn render_bundle_metadata(
    dest: &Path,
    executable: &Path,
    minimum_system_version: &str,
    digest: [u8; 32],
) -> Result<BundleMetadata, FormatError> {
    let display_name = dest
        .file_stem()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or("Squallz SFX");
    let executable_name = executable
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| FormatError::Unsupported("app executable name is not UTF-8".into()))?;
    let digest_prefix = digest[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let plist = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n  <key>CFBundleDisplayName</key><string>{}</string>\n  <key>CFBundleName</key><string>{}</string>\n  <key>CFBundleIdentifier</key><string>dev.squallz.sfx.{digest_prefix}</string>\n  <key>CFBundleExecutable</key><string>{}</string>\n  <key>CFBundlePackageType</key><string>APPL</string>\n  <key>CFBundleVersion</key><string>1</string>\n  <key>CFBundleShortVersionString</key><string>1.0</string>\n  <key>CFBundleIconFile</key><string>icon.icns</string>\n  <key>LSApplicationCategoryType</key><string>public.app-category.utilities</string>\n  <key>LSMinimumSystemVersion</key><string>{}</string>\n  <key>NSHighResolutionCapable</key><true/>\n  <key>CFBundleAllowMixedLocalizations</key><true/>\n  <key>CFBundleLocalizations</key><array><string>en</string><string>zh-Hans</string></array>\n</dict>\n</plist>\n",
        xml_escape(display_name),
        xml_escape(display_name),
        xml_escape(executable_name),
        xml_escape(minimum_system_version),
    );
    if usize_to_budget_bytes(plist.len())? > MAX_INFO_PLIST_BYTES {
        return Err(FormatError::ResourceLimitExceeded(
            "generated macOS SFX Info.plist exceeds 1 MiB".into(),
        ));
    }
    let localized = format!(
        "\"CFBundleDisplayName\" = \"{}\";\n\"CFBundleName\" = \"{}\";\n",
        strings_escape(display_name),
        strings_escape(display_name)
    );
    Ok(BundleMetadata { plist, localized })
}

fn write_bundle_metadata(
    tree: &BundleTree,
    prepared: &PreparedTemplate,
    metadata: &BundleMetadata,
) -> Result<(), FormatError> {
    write_tree_file(
        tree,
        Path::new("Contents/Info.plist"),
        metadata.plist.as_bytes(),
    )?;
    for locale in ["en.lproj", "zh-Hans.lproj"] {
        let dir = Path::new("Contents/Resources").join(locale);
        if prepared.entries.iter().any(|entry| {
            entry.relative == dir && matches!(entry.kind, TemplateEntryKind::Directory)
        }) {
            write_tree_file(
                tree,
                &dir.join("InfoPlist.strings"),
                metadata.localized.as_bytes(),
            )?;
        }
    }
    Ok(())
}

fn write_tree_file(tree: &BundleTree, relative: &Path, bytes: &[u8]) -> Result<(), FormatError> {
    let mut file = tree.rewrite_file(relative)?;
    file.write_all(bytes)?;
    set_generated_file_permissions(&file)?;
    file.sync_all()?;
    Ok(())
}

#[cfg(unix)]
fn set_generated_directory_permissions(
    tree: &BundleTree,
    relative: &Path,
) -> Result<(), FormatError> {
    use std::os::unix::fs::PermissionsExt;

    tree.set_permissions(relative, fs::Permissions::from_mode(0o755), true)?;
    Ok(())
}

#[cfg(not(unix))]
fn set_generated_directory_permissions(
    _tree: &BundleTree,
    _relative: &Path,
) -> Result<(), FormatError> {
    Ok(())
}

#[cfg(unix)]
fn set_generated_file_permissions(file: &File) -> Result<(), FormatError> {
    use std::os::unix::fs::PermissionsExt;

    file.set_permissions(fs::Permissions::from_mode(0o644))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_generated_file_permissions(_file: &File) -> Result<(), FormatError> {
    Ok(())
}

fn read_manifest(path: &Path) -> Result<BundleManifest, FormatError> {
    let mut file = File::open(path)?;
    let mut bytes = [0u8; MANIFEST_LEN];
    file.read_exact(&mut bytes)?;
    let mut extra = [0u8; 1];
    if file.read(&mut extra)? != 0 {
        return Err(FormatError::CorruptArchive(
            "macOS SFX bundle manifest has trailing data".into(),
        ));
    }
    BundleManifest::decode(&bytes)
}

fn sync_bundle_directories(
    prepared: &PreparedTemplate,
    tree: &BundleTree,
) -> Result<(), FormatError> {
    let order = bundle_directory_sync_order(prepared.entries.iter().filter_map(|entry| {
        matches!(entry.kind, TemplateEntryKind::Directory).then_some(entry.relative.as_path())
    }));
    for relative in order {
        tree.sync_dir(&relative)?;
    }
    Ok(())
}

fn bundle_directory_sync_order<'a>(
    template_directories: impl Iterator<Item = &'a Path>,
) -> Vec<PathBuf> {
    let mut directories = template_directories
        .map(Path::to_path_buf)
        .collect::<BTreeSet<_>>();
    directories.insert(PathBuf::from("Contents/Resources"));
    directories.insert(resource_relative_dir());
    directories.insert(PathBuf::new());
    let mut directories = directories.into_iter().collect::<Vec<_>>();
    directories.sort_by(|left, right| {
        right
            .components()
            .count()
            .cmp(&left.components().count())
            .then_with(|| left.cmp(right))
    });
    directories
}

fn directory_bytes(path: &Path) -> Result<u64, FormatError> {
    let mut total = 0u64;
    let mut entries = 0usize;
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            entries += 1;
            if entries > MAX_TEMPLATE_ENTRIES {
                return Err(FormatError::ResourceLimitExceeded(
                    "SFX bundle contains too many entries".into(),
                ));
            }
            let metadata = fs::symlink_metadata(entry.path())?;
            if metadata.is_dir() && !metadata.file_type().is_symlink() {
                stack.push(entry.path());
            } else if metadata.is_file() {
                total = total.checked_add(metadata.len()).ok_or_else(|| {
                    FormatError::ResourceLimitExceeded("SFX bundle size overflow".into())
                })?;
            }
        }
    }
    Ok(total)
}

fn validate_symlink(template: &Path, relative: &Path, target: &Path) -> Result<(), FormatError> {
    if target.is_absolute() {
        return Err(FormatError::Unsupported(format!(
            "app template symlink points outside the bundle: {}",
            relative.display()
        )));
    }
    let parent = relative.parent().unwrap_or_else(|| Path::new(""));
    let normalized = normalize_relative(&parent.join(target)).ok_or_else(|| {
        FormatError::Unsupported(format!(
            "app template symlink escapes the bundle: {}",
            relative.display()
        ))
    })?;
    let target_path = template.join(&normalized);
    if fs::symlink_metadata(&target_path).is_err() {
        return Err(FormatError::Unsupported(format!(
            "app template contains a dangling symlink: {}",
            relative.display()
        )));
    }
    let template_root = fs::canonicalize(template)?;
    let resolved = fs::canonicalize(target_path)?;
    if !resolved.starts_with(template_root) {
        return Err(FormatError::Unsupported(format!(
            "app template symlink resolves outside the bundle: {}",
            relative.display()
        )));
    }
    Ok(())
}

fn normalize_relative(path: &Path) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(value) => out.push(value),
            Component::ParentDir => {
                if !out.pop() {
                    return None;
                }
            }
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(out)
}

fn destination_is_inside_template(template: &Path, dest: &Path) -> Result<bool, FormatError> {
    let template = fs::canonicalize(template)?;
    let parent = dest
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let parent = fs::canonicalize(parent)?;
    Ok(parent
        .join(dest.file_name().unwrap_or_default())
        .starts_with(template))
}

fn require_regular_file(path: &Path, label: &str) -> Result<(), FormatError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            FormatError::CorruptArchive(format!("missing {label}"))
        } else {
            error.into()
        }
    })?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(FormatError::CorruptArchive(format!(
            "{label} must be a regular file"
        )));
    }
    Ok(())
}

fn is_app_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("app"))
}

fn resource_relative_dir() -> PathBuf {
    PathBuf::from("Contents/Resources").join(RESOURCE_DIR)
}

fn is_localized_info_plist(path: &Path) -> bool {
    path.file_name() == Some(OsStr::new("InfoPlist.strings"))
        && path
            .parent()
            .and_then(Path::extension)
            .and_then(OsStr::to_str)
            .is_some_and(|extension| extension.eq_ignore_ascii_case("lproj"))
}

fn resource_dir(bundle: &Path) -> PathBuf {
    bundle.join(resource_relative_dir())
}

fn payload_path(bundle: &Path) -> PathBuf {
    resource_dir(bundle).join(PAYLOAD_NAME)
}

fn manifest_path(bundle: &Path) -> PathBuf {
    resource_dir(bundle).join(MANIFEST_NAME)
}

fn copy_array<const N: usize>(bytes: &[u8]) -> Result<[u8; N], FormatError> {
    bytes.try_into().map_err(|_| {
        FormatError::CorruptArchive("truncated macOS SFX bundle manifest field".into())
    })
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn strings_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_round_trip_keeps_sha256_and_length() {
        let manifest = BundleManifest {
            payload_bytes: 42,
            payload_sha256: [0x5a; 32],
        };
        let parsed = BundleManifest::decode(&manifest.encode()).unwrap();
        assert_eq!(parsed.payload_bytes, 42);
        assert_eq!(parsed.payload_sha256, [0x5a; 32]);
    }

    #[test]
    fn relative_normalization_rejects_bundle_escape() {
        assert_eq!(
            normalize_relative(Path::new("Contents/Frameworks/../MacOS/tool")),
            Some(PathBuf::from("Contents/MacOS/tool"))
        );
        assert_eq!(normalize_relative(Path::new("../../outside")), None);
    }

    #[test]
    fn generated_bundle_entries_are_reserved_before_staging() {
        let generated_entries = 6;
        validate_bundle_entry_count(MAX_TEMPLATE_ENTRIES - generated_entries, generated_entries)
            .unwrap();
        let error = validate_bundle_entry_count(
            MAX_TEMPLATE_ENTRIES - generated_entries + 1,
            generated_entries,
        )
        .unwrap_err();
        assert!(matches!(error, FormatError::ResourceLimitExceeded(_)));
    }

    #[test]
    fn desktop_quick_look_extension_is_not_copied_into_sfx_bundles() {
        let extension = Path::new(DESKTOP_QUICK_LOOK_EXTENSION);

        assert!(is_desktop_quick_look_extension(extension));
        assert!(is_desktop_quick_look_extension(
            &extension.join("Contents/MacOS/SquallzQuickLook")
        ));
        assert!(!is_desktop_quick_look_extension(Path::new(
            "Contents/PlugIns/AnotherExtension.appex"
        )));
    }

    #[test]
    fn entry_budget_covers_directories_links_paths_and_node_metadata() {
        let allocation = 4096;
        assert_eq!(
            entry_output_budget(Path::new("Contents/Resources"), 0, allocation).unwrap(),
            allocation * 3
        );
        assert_eq!(
            entry_output_budget(Path::new("Contents/Resources/current"), 9, allocation).unwrap(),
            allocation * 4
        );
        let long_path = PathBuf::from("x".repeat(allocation as usize + 1));
        assert_eq!(
            entry_output_budget(&long_path, 0, allocation).unwrap(),
            allocation * 4
        );
    }

    #[test]
    fn generated_metadata_rejects_an_unbounded_rendered_plist() {
        let minimum_version = "&".repeat((MAX_INFO_PLIST_BYTES / 4) as usize);
        let error = render_bundle_metadata(
            Path::new("Package.app"),
            Path::new("squallz-gui"),
            &minimum_version,
            [0u8; 32],
        )
        .unwrap_err();
        assert!(matches!(error, FormatError::ResourceLimitExceeded(_)));
    }

    #[test]
    fn digest_value_does_not_change_generated_metadata_size() {
        let empty = render_bundle_metadata(
            Path::new("Package.app"),
            Path::new("squallz-gui"),
            "11.0",
            [0u8; 32],
        )
        .unwrap();
        let full = render_bundle_metadata(
            Path::new("Package.app"),
            Path::new("squallz-gui"),
            "11.0",
            [0xff; 32],
        )
        .unwrap();
        assert_eq!(empty.plist.len(), full.plist.len());
        assert_eq!(empty.localized.len(), full.localized.len());
    }

    #[test]
    fn staging_reservation_does_not_reuse_an_existing_directory() {
        let dest = std::env::temp_dir().join(format!(
            "sqz-sfx-bundle-reservation-{}.app",
            std::process::id()
        ));
        let (collision, collision_identity) =
            super::super::transaction::reserve_staged_path(&dest, SfxLayout::MacosApp).unwrap();
        fs::write(collision.join("owned-by-another-task"), b"keep").unwrap();

        let (reserved, reserved_identity) =
            super::super::transaction::reserve_staged_path(&dest, SfxLayout::MacosApp).unwrap();

        assert_ne!(reserved, collision);
        assert_eq!(
            fs::read(collision.join("owned-by-another-task")).unwrap(),
            b"keep"
        );
        super::super::transaction::discard_staged_path(
            &reserved,
            reserved_identity,
            SfxLayout::MacosApp,
            &dest,
        )
        .unwrap();
        super::super::transaction::discard_staged_path(
            &collision,
            collision_identity,
            SfxLayout::MacosApp,
            &dest,
        )
        .unwrap();
    }

    #[test]
    fn durability_sync_covers_every_bundle_directory_bottom_up() {
        let directories = [
            Path::new("Contents"),
            Path::new("Contents/MacOS"),
            Path::new("Contents/Resources"),
        ];
        let order = bundle_directory_sync_order(directories.into_iter());
        let position = |path: &Path| {
            order
                .iter()
                .position(|candidate| candidate == path)
                .unwrap()
        };

        assert!(order.contains(&resource_relative_dir()));
        assert!(order.contains(&PathBuf::new()));
        assert!(position(Path::new("Contents/MacOS")) < position(Path::new("Contents")));
        assert!(position(&resource_relative_dir()) < position(Path::new("Contents/Resources")));
        assert!(position(Path::new("Contents/Resources")) < position(Path::new("Contents")));
        assert!(position(Path::new("Contents")) < position(Path::new("")));
    }
}
