//! Stable-channel update discovery shared by Squallz desktop and CLI clients.
//!
//! This module only reads release metadata. Installing an update remains an
//! explicit browser handoff until every published package has a verified
//! platform-signing path.

use std::fmt;
use std::time::Duration;

use reqwest::header::{ACCEPT, CONTENT_LENGTH};
use semver::Version;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

const LATEST_RELEASE_API: &str = "https://api.github.com/repos/yangzhg/Squallz/releases/latest";
const LATEST_RELEASE_URL: &str = "https://github.com/yangzhg/Squallz/releases/latest";
const RELEASE_BASE_URL: &str = "https://github.com/yangzhg/Squallz/releases";
const DOWNLOAD_BASE_URL: &str = "https://github.com/yangzhg/Squallz/releases/download";
const RELEASE_MANIFEST_NAME: &str = "RELEASE_ASSETS_MANIFEST.json";
const RELEASE_MANIFEST_SCHEMA: &str = "dev.squallz.release.manifest.v1";
const RELEASE_TRUST_SCHEMA: &str = "dev.squallz.macos.release-trust.v1";
const RELEASE_REPOSITORY: &str = "yangzhg/Squallz";
const MAX_RELEASE_RESPONSE_BYTES: usize = 1024 * 1024;
const MAX_MANIFEST_RESPONSE_BYTES: usize = 256 * 1024;
const MAX_TRUST_RESPONSE_BYTES: usize = 256 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(12);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateStatus {
    UpToDate,
    UpdateAvailable,
    Ahead,
}

impl UpdateStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UpToDate => "up_to_date",
            Self::UpdateAvailable => "update_available",
            Self::Ahead => "ahead",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateTrust {
    DeveloperIdNotarized,
    UnsignedPreview,
    Unavailable,
}

impl UpdateTrust {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DeveloperIdNotarized => "developer_id_notarized",
            Self::UnsignedPreview => "unsigned_preview",
            Self::Unavailable => "unavailable",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateMetadataSource {
    GithubApi,
    LatestReleaseRedirect,
    LatestReleaseManifest,
}

impl UpdateMetadataSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::GithubApi => "github_api",
            Self::LatestReleaseRedirect => "latest_release_redirect",
            Self::LatestReleaseManifest => "latest_release_manifest",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleasePackage {
    Desktop,
    CommandLine,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateErrorKind {
    Network,
    RateLimited,
    InvalidResponse,
    NoRelease,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateError {
    kind: UpdateErrorKind,
    detail: String,
}

impl UpdateError {
    pub fn unavailable(detail: impl Into<String>) -> Self {
        Self {
            kind: UpdateErrorKind::Unavailable,
            detail: detail.into(),
        }
    }

    pub const fn kind(&self) -> UpdateErrorKind {
        self.kind
    }

    pub const fn i18n_key(&self) -> &'static str {
        match self.kind {
            UpdateErrorKind::Network => "error.update.network",
            UpdateErrorKind::RateLimited => "error.update.rate_limited",
            UpdateErrorKind::InvalidResponse => "error.update.invalid_response",
            UpdateErrorKind::NoRelease => "error.update.no_release",
            UpdateErrorKind::Unavailable => "error.update.unavailable",
        }
    }

    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl fmt::Display for UpdateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.detail)
    }
}

impl std::error::Error for UpdateError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCheck {
    pub status: UpdateStatus,
    pub current_version: String,
    pub latest_version: String,
    pub release_name: String,
    pub release_url: String,
    pub published_at: String,
    pub platform: String,
    pub architecture: String,
    pub asset_name: Option<String>,
    pub download_url: Option<String>,
    pub asset_size_bytes: Option<u64>,
    pub asset_sha256: Option<String>,
    pub asset_trust: UpdateTrust,
    pub metadata_source: UpdateMetadataSource,
}

#[derive(Debug, Clone, Deserialize)]
struct GithubRelease {
    tag_name: String,
    name: Option<String>,
    html_url: String,
    draft: bool,
    prerelease: bool,
    published_at: Option<String>,
    assets: Vec<GithubAsset>,
}

#[derive(Debug, Clone, Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
    size: u64,
    digest: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ReleaseManifest {
    schema: String,
    repository: String,
    version: String,
    assets: Vec<ReleaseManifestAsset>,
}

#[derive(Debug, Deserialize)]
struct ReleaseManifestAsset {
    name: String,
    sha256: String,
    size_bytes: u64,
    trust_state: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ReleaseTrustEvidence {
    schema: String,
    status: String,
    packaging_valid: bool,
    architecture: String,
    notarization: ReleaseTrustNotarization,
    stapled: bool,
    gatekeeper: bool,
    artifact: ReleaseTrustArtifact,
}

#[derive(Debug, Deserialize)]
struct ReleaseTrustNotarization {
    status: String,
}

#[derive(Debug, Deserialize)]
struct ReleaseTrustArtifact {
    #[serde(rename = "name")]
    _name: String,
    sha256: String,
    size_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ReleaseTarget {
    platform: &'static str,
    architecture: &'static str,
    asset_platform: Option<&'static str>,
    package: ReleasePackage,
}

/// Reads the latest non-draft, non-prerelease GitHub Release and resolves the
/// requested package for this build target. No software package is downloaded
/// or installed.
pub async fn check_for_updates(
    current_version: &str,
    package: ReleasePackage,
) -> Result<UpdateCheck, UpdateError> {
    let current_version = normalize_current_version(current_version)?;
    let target = current_release_target(package);
    let client = reqwest::Client::builder()
        .https_only(true)
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .user_agent(format!("Squallz/{current_version} update-check"))
        .build()
        .map_err(|error| update_error(UpdateErrorKind::Unavailable, error.to_string()))?;

    let response = client
        .get(LATEST_RELEASE_API)
        .header(ACCEPT, "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .send()
        .await
        .map_err(|error| update_error(UpdateErrorKind::Network, error.to_string()))?;

    let status = response.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Err(update_error(
            UpdateErrorKind::NoRelease,
            "GitHub returned no stable Squallz release",
        ));
    }
    if status == reqwest::StatusCode::FORBIDDEN || status == reqwest::StatusCode::TOO_MANY_REQUESTS
    {
        if let Some(update) = check_latest_release_redirect(&client, &current_version, target).await
        {
            return Ok(update);
        }
        return Err(update_error(
            UpdateErrorKind::RateLimited,
            format!("GitHub update request returned {status}"),
        ));
    }
    if !status.is_success() {
        return Err(update_error(
            UpdateErrorKind::Unavailable,
            format!("GitHub update request returned {status}"),
        ));
    }

    let bytes = read_bounded_response(response, MAX_RELEASE_RESPONSE_BYTES)
        .await
        .map_err(|error| match error {
            BoundedResponseError::TooLarge => update_error(
                UpdateErrorKind::InvalidResponse,
                "GitHub release response exceeds the update metadata limit",
            ),
            BoundedResponseError::Network(error) => {
                update_error(UpdateErrorKind::Network, error.to_string())
            }
        })?;
    let release: GithubRelease = serde_json::from_slice(&bytes)
        .map_err(|error| update_error(UpdateErrorKind::InvalidResponse, error.to_string()))?;

    validate_release_identity(&release)?;
    let preliminary = evaluate_release(release.clone(), &current_version, target, None, None)?;
    if preliminary.status != UpdateStatus::UpdateAvailable || preliminary.asset_name.is_none() {
        return Ok(preliminary);
    }

    let asset = select_asset(&release, target);
    let manifest = if asset.is_some() {
        fetch_release_manifest(&client, &release).await
    } else {
        None
    };
    let trust_evidence = if requires_release_trust_evidence(target, asset.as_ref()) {
        let trust_evidence = match asset.as_ref() {
            Some(asset) => fetch_release_trust_evidence(&client, &release, asset).await,
            None => None,
        };
        trust_evidence
    } else {
        None
    };
    evaluate_release(
        release,
        &current_version,
        target,
        manifest.as_ref(),
        trust_evidence.as_ref(),
    )
}

fn normalize_current_version(current_version: &str) -> Result<String, UpdateError> {
    Version::parse(current_version)
        .map(|version| version.to_string())
        .map_err(|error| {
            update_error(
                UpdateErrorKind::InvalidResponse,
                format!("installed Squallz version is not semantic versioning: {error}"),
            )
        })
}

fn evaluate_release(
    release: GithubRelease,
    current_version: &str,
    target: ReleaseTarget,
    manifest: Option<&ReleaseManifest>,
    trust_evidence: Option<&ReleaseTrustEvidence>,
) -> Result<UpdateCheck, UpdateError> {
    evaluate_release_from_source(
        release,
        current_version,
        target,
        manifest,
        trust_evidence,
        UpdateMetadataSource::GithubApi,
    )
}

fn evaluate_release_from_source(
    release: GithubRelease,
    current_version: &str,
    target: ReleaseTarget,
    manifest: Option<&ReleaseManifest>,
    trust_evidence: Option<&ReleaseTrustEvidence>,
    metadata_source: UpdateMetadataSource,
) -> Result<UpdateCheck, UpdateError> {
    validate_release_identity(&release)?;

    let latest_version_text = release.tag_name.strip_prefix('v').ok_or_else(|| {
        update_error(
            UpdateErrorKind::InvalidResponse,
            "GitHub release tag is missing its v prefix",
        )
    })?;
    let latest_version = Version::parse(latest_version_text).map_err(|error| {
        update_error(
            UpdateErrorKind::InvalidResponse,
            format!("GitHub release tag is not semantic versioning: {error}"),
        )
    })?;
    if !latest_version.pre.is_empty() {
        return Err(update_error(
            UpdateErrorKind::InvalidResponse,
            "GitHub latest release tag is a prerelease",
        ));
    }
    let installed_version = Version::parse(current_version).map_err(|error| {
        update_error(
            UpdateErrorKind::InvalidResponse,
            format!("installed app version is not semantic versioning: {error}"),
        )
    })?;
    let expected_release_url = format!("{RELEASE_BASE_URL}/tag/{}", release.tag_name);

    let status = match latest_version.cmp_precedence(&installed_version) {
        std::cmp::Ordering::Greater => UpdateStatus::UpdateAvailable,
        std::cmp::Ordering::Equal => UpdateStatus::UpToDate,
        std::cmp::Ordering::Less => UpdateStatus::Ahead,
    };
    let asset = (status == UpdateStatus::UpdateAvailable)
        .then(|| select_asset(&release, target))
        .flatten();
    let metadata_conflict = asset
        .as_ref()
        .is_some_and(|asset| release_asset_metadata_conflicts(asset, &release, manifest));
    let asset_trust = if metadata_conflict {
        UpdateTrust::Unavailable
    } else {
        asset
            .as_ref()
            .map(|asset| release_asset_trust(asset, &release, manifest, trust_evidence, target))
            .unwrap_or(UpdateTrust::Unavailable)
    };
    let asset_sha256 = if metadata_conflict {
        None
    } else {
        asset
            .as_ref()
            .and_then(|asset| release_asset_sha256(asset, &release, manifest))
    };

    Ok(UpdateCheck {
        status,
        current_version: installed_version.to_string(),
        latest_version: latest_version.to_string(),
        release_name: bounded_release_name(release.name.as_deref(), &release.tag_name),
        release_url: expected_release_url,
        published_at: release
            .published_at
            .filter(|value| value.len() <= 64)
            .unwrap_or_default(),
        platform: target.platform.to_owned(),
        architecture: target.architecture.to_owned(),
        asset_name: asset.as_ref().map(|asset| asset.name.clone()),
        download_url: asset
            .as_ref()
            .filter(|_| !metadata_conflict)
            .map(|asset| asset.browser_download_url.clone()),
        asset_size_bytes: asset.as_ref().map(|asset| asset.size),
        asset_sha256,
        asset_trust,
        metadata_source,
    })
}

async fn check_latest_release_redirect(
    client: &reqwest::Client,
    current_version: &str,
    target: ReleaseTarget,
) -> Option<UpdateCheck> {
    let response = client.head(LATEST_RELEASE_URL).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let release = release_from_latest_redirect(response.url()).ok()?;
    let preliminary = evaluate_release_from_source(
        release.clone(),
        current_version,
        target,
        None,
        None,
        UpdateMetadataSource::LatestReleaseRedirect,
    )
    .ok()?;
    if preliminary.status != UpdateStatus::UpdateAvailable || target.asset_platform.is_none() {
        return Some(preliminary);
    }

    let Some(manifest) = fetch_release_manifest_by_name(client, &release).await else {
        return Some(preliminary);
    };
    let Some(asset) = manifest_asset_for_target(&release, &manifest, target) else {
        return Some(preliminary);
    };
    let mut enriched_release = release;
    enriched_release.assets.push(asset);
    evaluate_release_from_source(
        enriched_release,
        current_version,
        target,
        Some(&manifest),
        None,
        UpdateMetadataSource::LatestReleaseManifest,
    )
    .ok()
    .or(Some(preliminary))
}

fn release_from_latest_redirect(url: &reqwest::Url) -> Result<GithubRelease, UpdateError> {
    if url.scheme() != "https"
        || url.host_str() != Some("github.com")
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(update_error(
            UpdateErrorKind::InvalidResponse,
            "GitHub latest release redirect did not resolve to an exact release URL",
        ));
    }

    let tag_name = url
        .path()
        .strip_prefix("/yangzhg/Squallz/releases/tag/")
        .filter(|tag| !tag.is_empty() && !tag.contains('/'))
        .ok_or_else(|| {
            update_error(
                UpdateErrorKind::InvalidResponse,
                "GitHub latest release redirect did not resolve to the Squallz repository",
            )
        })?;
    let version_text = tag_name.strip_prefix('v').ok_or_else(|| {
        update_error(
            UpdateErrorKind::InvalidResponse,
            "GitHub latest release redirect tag is missing its v prefix",
        )
    })?;
    let version = Version::parse(version_text).map_err(|error| {
        update_error(
            UpdateErrorKind::InvalidResponse,
            format!("GitHub latest release redirect tag is not semantic versioning: {error}"),
        )
    })?;
    if !version.pre.is_empty() {
        return Err(update_error(
            UpdateErrorKind::InvalidResponse,
            "GitHub latest release redirect resolved to a prerelease tag",
        ));
    }

    let expected_release_url = format!("{RELEASE_BASE_URL}/tag/{tag_name}");
    if url.as_str() != expected_release_url {
        return Err(update_error(
            UpdateErrorKind::InvalidResponse,
            "GitHub latest release redirect URL was not canonical",
        ));
    }

    Ok(GithubRelease {
        tag_name: tag_name.to_owned(),
        name: None,
        html_url: expected_release_url,
        draft: false,
        prerelease: false,
        published_at: None,
        assets: Vec::new(),
    })
}

fn validate_release_identity(release: &GithubRelease) -> Result<(), UpdateError> {
    if release.draft || release.prerelease {
        return Err(update_error(
            UpdateErrorKind::InvalidResponse,
            "GitHub latest release is not a stable published release",
        ));
    }
    let expected_release_url = format!("{RELEASE_BASE_URL}/tag/{}", release.tag_name);
    if release.html_url != expected_release_url {
        return Err(update_error(
            UpdateErrorKind::InvalidResponse,
            "GitHub release URL did not match the Squallz repository",
        ));
    }
    Ok(())
}

async fn fetch_release_manifest(
    client: &reqwest::Client,
    release: &GithubRelease,
) -> Option<ReleaseManifest> {
    let expected_url = format!(
        "{DOWNLOAD_BASE_URL}/{}/{}",
        release.tag_name, RELEASE_MANIFEST_NAME
    );
    let manifest_asset = release.assets.iter().find(|asset| {
        asset.name == RELEASE_MANIFEST_NAME && asset.browser_download_url == expected_url
    })?;
    if manifest_asset.size > MAX_MANIFEST_RESPONSE_BYTES as u64 {
        return None;
    }

    fetch_bounded_json(
        client,
        &manifest_asset.browser_download_url,
        MAX_MANIFEST_RESPONSE_BYTES,
    )
    .await
}

async fn fetch_release_manifest_by_name(
    client: &reqwest::Client,
    release: &GithubRelease,
) -> Option<ReleaseManifest> {
    let expected_url = format!(
        "{DOWNLOAD_BASE_URL}/{}/{}",
        release.tag_name, RELEASE_MANIFEST_NAME
    );
    fetch_bounded_json(client, &expected_url, MAX_MANIFEST_RESPONSE_BYTES).await
}

async fn fetch_release_trust_evidence(
    client: &reqwest::Client,
    release: &GithubRelease,
    asset: &GithubAsset,
) -> Option<ReleaseTrustEvidence> {
    let trust_name = format!("{}.trust.json", asset.name);
    let expected_url = format!("{DOWNLOAD_BASE_URL}/{}/{}", release.tag_name, trust_name);
    let trust_asset = release.assets.iter().find(|candidate| {
        candidate.name == trust_name && candidate.browser_download_url == expected_url
    })?;
    if trust_asset.size > MAX_TRUST_RESPONSE_BYTES as u64 {
        return None;
    }

    fetch_bounded_json(
        client,
        &trust_asset.browser_download_url,
        MAX_TRUST_RESPONSE_BYTES,
    )
    .await
}

async fn fetch_bounded_json<T: DeserializeOwned>(
    client: &reqwest::Client,
    url: &str,
    max_response_bytes: usize,
) -> Option<T> {
    let response = client
        .get(url)
        .header(ACCEPT, "application/octet-stream")
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let bytes = read_bounded_response(response, max_response_bytes)
        .await
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

#[derive(Debug)]
enum BoundedResponseError {
    TooLarge,
    Network(reqwest::Error),
}

async fn read_bounded_response(
    mut response: reqwest::Response,
    max_response_bytes: usize,
) -> Result<Vec<u8>, BoundedResponseError> {
    if response
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok())
        .is_some_and(|length| length > max_response_bytes)
    {
        return Err(BoundedResponseError::TooLarge);
    }

    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(BoundedResponseError::Network)?
    {
        append_bounded(&mut body, &chunk, max_response_bytes)?;
    }
    Ok(body)
}

fn append_bounded(
    body: &mut Vec<u8>,
    chunk: &[u8],
    max_response_bytes: usize,
) -> Result<(), BoundedResponseError> {
    if chunk.len() > max_response_bytes.saturating_sub(body.len()) {
        return Err(BoundedResponseError::TooLarge);
    }
    body.extend_from_slice(chunk);
    Ok(())
}

fn select_asset(release: &GithubRelease, target: ReleaseTarget) -> Option<GithubAsset> {
    expected_asset_names(release, target)
        .into_iter()
        .find_map(|expected_name| {
            let expected_url =
                format!("{DOWNLOAD_BASE_URL}/{}/{}", release.tag_name, expected_name);
            release
                .assets
                .iter()
                .find(|asset| {
                    asset.name == expected_name && asset.browser_download_url == expected_url
                })
                .cloned()
        })
}

fn expected_asset_names(release: &GithubRelease, target: ReleaseTarget) -> Vec<String> {
    let Some(asset_platform) = target.asset_platform else {
        return Vec::new();
    };
    if target.platform == "macos" {
        vec![
            format!("Squallz-{}-{asset_platform}.dmg", release.tag_name),
            format!("Squallz-{}-{asset_platform}.app.zip", release.tag_name),
        ]
    } else if target.platform == "windows" {
        let product = match target.package {
            ReleasePackage::Desktop => "Squallz",
            ReleasePackage::CommandLine => "sqz",
        };
        vec![format!(
            "{product}-{}-{asset_platform}.exe",
            release.tag_name
        )]
    } else {
        let product = match target.package {
            ReleasePackage::Desktop => "Squallz",
            ReleasePackage::CommandLine => "sqz",
        };
        vec![format!(
            "{product}-{}-{asset_platform}.tar.gz",
            release.tag_name
        )]
    }
}

fn manifest_asset_for_target(
    release: &GithubRelease,
    manifest: &ReleaseManifest,
    target: ReleaseTarget,
) -> Option<GithubAsset> {
    if manifest.schema != RELEASE_MANIFEST_SCHEMA
        || manifest.repository != RELEASE_REPOSITORY
        || manifest.version != release.tag_name
    {
        return None;
    }

    for expected_name in expected_asset_names(release, target) {
        let mut records = manifest
            .assets
            .iter()
            .filter(|record| record.name == expected_name);
        let Some(record) = records.next() else {
            continue;
        };
        if records.next().is_some() || normalized_hex_sha256(&record.sha256).is_none() {
            return None;
        }
        return Some(GithubAsset {
            browser_download_url: format!(
                "{DOWNLOAD_BASE_URL}/{}/{}",
                release.tag_name, expected_name
            ),
            name: expected_name,
            size: record.size_bytes,
            digest: None,
        });
    }
    None
}

fn release_asset_trust(
    asset: &GithubAsset,
    release: &GithubRelease,
    manifest: Option<&ReleaseManifest>,
    trust_evidence: Option<&ReleaseTrustEvidence>,
    target: ReleaseTarget,
) -> UpdateTrust {
    if target.platform != "macos" || !asset.name.ends_with(".dmg") {
        return UpdateTrust::UnsignedPreview;
    }

    let Some(manifest) = manifest else {
        return UpdateTrust::UnsignedPreview;
    };
    let Some(record) = release_manifest_asset(manifest, release, asset) else {
        return UpdateTrust::UnsignedPreview;
    };
    let github_digest = normalized_sha256(asset.digest.as_deref());
    let manifest_digest = normalized_hex_sha256(&record.sha256);
    if record.size_bytes != asset.size
        || record.trust_state.as_deref() != Some("developer-id-notarized")
        || github_digest.is_none()
        || github_digest != manifest_digest
    {
        return UpdateTrust::UnsignedPreview;
    }

    let Some(evidence) = trust_evidence else {
        return UpdateTrust::UnsignedPreview;
    };
    if evidence.schema != RELEASE_TRUST_SCHEMA
        || evidence.status != "pass"
        || !evidence.packaging_valid
        || evidence.architecture != target.architecture
        || evidence.notarization.status != "Accepted"
        || !evidence.stapled
        || !evidence.gatekeeper
        || evidence.artifact.size_bytes != asset.size
        || normalized_hex_sha256(&evidence.artifact.sha256) != github_digest
    {
        return UpdateTrust::UnsignedPreview;
    }

    UpdateTrust::DeveloperIdNotarized
}

fn release_asset_sha256(
    asset: &GithubAsset,
    release: &GithubRelease,
    manifest: Option<&ReleaseManifest>,
) -> Option<String> {
    let github_digest = normalized_sha256(asset.digest.as_deref());
    let manifest_digest = manifest
        .and_then(|manifest| release_manifest_asset(manifest, release, asset))
        .and_then(|record| normalized_hex_sha256(&record.sha256));

    match (github_digest, manifest_digest) {
        (Some(github), Some(manifest)) if github != manifest => None,
        (Some(github), _) => Some(github),
        (None, Some(manifest)) => Some(manifest),
        (None, None) => None,
    }
}

fn release_asset_metadata_conflicts(
    asset: &GithubAsset,
    release: &GithubRelease,
    manifest: Option<&ReleaseManifest>,
) -> bool {
    if asset.digest.is_some() && normalized_sha256(asset.digest.as_deref()).is_none() {
        return true;
    }
    let Some(manifest) = manifest else {
        return false;
    };
    if manifest.schema != RELEASE_MANIFEST_SCHEMA
        || manifest.repository != RELEASE_REPOSITORY
        || manifest.version != release.tag_name
    {
        return false;
    }
    let mut records = manifest
        .assets
        .iter()
        .filter(|record| record.name == asset.name);
    let Some(record) = records.next() else {
        return true;
    };
    if records.next().is_some() {
        return true;
    }
    if record.size_bytes != asset.size {
        return true;
    }
    let Some(manifest_digest) = normalized_hex_sha256(&record.sha256) else {
        return true;
    };
    match asset.digest.as_deref() {
        Some(digest) => {
            normalized_sha256(Some(digest)).as_deref() != Some(manifest_digest.as_str())
        }
        None => false,
    }
}

fn release_manifest_asset<'a>(
    manifest: &'a ReleaseManifest,
    release: &GithubRelease,
    asset: &GithubAsset,
) -> Option<&'a ReleaseManifestAsset> {
    if manifest.schema != RELEASE_MANIFEST_SCHEMA
        || manifest.repository != RELEASE_REPOSITORY
        || manifest.version != release.tag_name
    {
        return None;
    }
    let mut records = manifest
        .assets
        .iter()
        .filter(|record| record.name == asset.name);
    let record = records.next()?;
    if records.next().is_some() || record.size_bytes != asset.size {
        return None;
    }
    Some(record)
}

fn requires_release_trust_evidence(target: ReleaseTarget, asset: Option<&GithubAsset>) -> bool {
    target.platform == "macos"
        && asset
            .as_ref()
            .is_some_and(|asset| asset.name.ends_with(".dmg"))
}

fn normalized_sha256(value: Option<&str>) -> Option<String> {
    let digest = value?.strip_prefix("sha256:")?;
    normalized_hex_sha256(digest)
}

fn normalized_hex_sha256(digest: &str) -> Option<String> {
    (digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| digest.to_ascii_lowercase())
}

fn bounded_release_name(name: Option<&str>, tag: &str) -> String {
    let candidate = name.map(str::trim).filter(|value| !value.is_empty());
    match candidate {
        Some(value) if value.chars().count() <= 120 => value.to_owned(),
        _ => format!("Squallz {tag}"),
    }
}

fn current_release_target(package: ReleasePackage) -> ReleaseTarget {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => ReleaseTarget {
            platform: "macos",
            architecture: "arm64",
            asset_platform: Some("macos-arm64"),
            package,
        },
        ("macos", "x86_64") => ReleaseTarget {
            platform: "macos",
            architecture: "x64",
            asset_platform: Some("macos-x64"),
            package,
        },
        ("windows", "x86_64") => ReleaseTarget {
            platform: "windows",
            architecture: "x64",
            asset_platform: Some("windows-x64"),
            package,
        },
        ("linux", "x86_64") => ReleaseTarget {
            platform: "linux",
            architecture: "x64",
            asset_platform: Some("linux-x64"),
            package,
        },
        (platform, architecture) => ReleaseTarget {
            platform,
            architecture,
            asset_platform: None,
            package,
        },
    }
}

fn update_error(kind: UpdateErrorKind, detail: impl Into<String>) -> UpdateError {
    UpdateError {
        kind,
        detail: detail.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(platform: &'static str, architecture: &'static str) -> ReleaseTarget {
        ReleaseTarget {
            platform,
            architecture,
            asset_platform: Some(match (platform, architecture) {
                ("macos", "arm64") => "macos-arm64",
                ("macos", "x64") => "macos-x64",
                ("windows", _) => "windows-x64",
                _ => "linux-x64",
            }),
            package: ReleasePackage::Desktop,
        }
    }

    fn command_line_target(platform: &'static str, architecture: &'static str) -> ReleaseTarget {
        ReleaseTarget {
            package: ReleasePackage::CommandLine,
            ..target(platform, architecture)
        }
    }

    fn asset(name: &str, digest: Option<&str>) -> GithubAsset {
        GithubAsset {
            name: name.to_owned(),
            browser_download_url: format!("{DOWNLOAD_BASE_URL}/v0.2.0/{name}"),
            size: 42,
            digest: digest.map(str::to_owned),
        }
    }

    fn release(assets: Vec<GithubAsset>) -> GithubRelease {
        GithubRelease {
            tag_name: "v0.2.0".to_owned(),
            name: Some("Squallz v0.2.0".to_owned()),
            html_url: format!("{RELEASE_BASE_URL}/tag/v0.2.0"),
            draft: false,
            prerelease: false,
            published_at: Some("2026-07-28T12:00:00Z".to_owned()),
            assets,
        }
    }

    fn signed_manifest(package_name: &str, digest: &str) -> ReleaseManifest {
        ReleaseManifest {
            schema: RELEASE_MANIFEST_SCHEMA.to_owned(),
            repository: RELEASE_REPOSITORY.to_owned(),
            version: "v0.2.0".to_owned(),
            assets: vec![ReleaseManifestAsset {
                name: package_name.to_owned(),
                sha256: digest.to_owned(),
                size_bytes: 42,
                trust_state: Some("developer-id-notarized".to_owned()),
            }],
        }
    }

    fn unsigned_manifest(package_name: &str, digest: &str) -> ReleaseManifest {
        let mut manifest = signed_manifest(package_name, digest);
        manifest.assets[0].trust_state = Some("unsigned-preview".to_owned());
        manifest
    }

    fn signed_trust_evidence(package_name: &str, digest: &str) -> ReleaseTrustEvidence {
        ReleaseTrustEvidence {
            schema: RELEASE_TRUST_SCHEMA.to_owned(),
            status: "pass".to_owned(),
            packaging_valid: true,
            architecture: "arm64".to_owned(),
            notarization: ReleaseTrustNotarization {
                status: "Accepted".to_owned(),
            },
            stapled: true,
            gatekeeper: true,
            artifact: ReleaseTrustArtifact {
                _name: package_name.to_owned(),
                sha256: digest.to_owned(),
                size_bytes: 42,
            },
        }
    }

    #[test]
    fn update_available_selects_the_exact_macos_package_and_digest() {
        let digest = "a".repeat(64);
        let package = asset(
            "Squallz-v0.2.0-macos-arm64.dmg",
            Some(&format!("sha256:{digest}")),
        );
        let trust = asset("Squallz-v0.2.0-macos-arm64.dmg.trust.json", None);
        let manifest = signed_manifest("Squallz-v0.2.0-macos-arm64.dmg", &digest);
        let evidence = signed_trust_evidence("Squallz-v0.2.0-macos-arm64.dmg", &digest);
        let result = evaluate_release(
            release(vec![package, trust]),
            "0.1.0",
            target("macos", "arm64"),
            Some(&manifest),
            Some(&evidence),
        )
        .expect("valid fixture should resolve");

        assert_eq!(result.status, UpdateStatus::UpdateAvailable);
        assert_eq!(
            result.asset_name.as_deref(),
            Some("Squallz-v0.2.0-macos-arm64.dmg")
        );
        assert_eq!(result.asset_sha256.as_deref(), Some(digest.as_str()));
        assert_eq!(result.asset_trust, UpdateTrust::DeveloperIdNotarized);
        assert_eq!(result.metadata_source, UpdateMetadataSource::GithubApi);
    }

    #[test]
    fn desktop_json_contract_keeps_camel_case_fields_and_snake_case_values() {
        let result = evaluate_release(
            release(Vec::new()),
            "0.1.0",
            target("linux", "x64"),
            None,
            None,
        )
        .expect("fixture should produce update metadata");

        let value = serde_json::to_value(result).expect("update metadata should serialize");

        assert_eq!(value["status"], "update_available");
        assert_eq!(value["currentVersion"], "0.1.0");
        assert_eq!(value["latestVersion"], "0.2.0");
        assert_eq!(value["assetTrust"], "unavailable");
        assert_eq!(value["metadataSource"], "github_api");
        assert!(value.get("current_version").is_none());
    }

    #[test]
    fn package_role_selects_the_cli_artifact_without_changing_macos_distribution() {
        let release = release(vec![
            asset("Squallz-v0.2.0-windows-x64.exe", None),
            asset("sqz-v0.2.0-windows-x64.exe", None),
            asset("Squallz-v0.2.0-linux-x64.tar.gz", None),
            asset("sqz-v0.2.0-linux-x64.tar.gz", None),
            asset("Squallz-v0.2.0-macos-arm64.dmg", None),
        ]);

        let cases = [
            (target("windows", "x64"), "Squallz-v0.2.0-windows-x64.exe"),
            (
                command_line_target("windows", "x64"),
                "sqz-v0.2.0-windows-x64.exe",
            ),
            (target("linux", "x64"), "Squallz-v0.2.0-linux-x64.tar.gz"),
            (
                command_line_target("linux", "x64"),
                "sqz-v0.2.0-linux-x64.tar.gz",
            ),
            (
                command_line_target("macos", "arm64"),
                "Squallz-v0.2.0-macos-arm64.dmg",
            ),
        ];

        for (target, expected_name) in cases {
            assert_eq!(
                select_asset(&release, target).map(|asset| asset.name),
                Some(expected_name.to_owned())
            );
        }
    }

    #[test]
    fn bounded_append_rejects_an_overflow_before_growing_the_buffer() {
        let mut body = b"1234".to_vec();
        append_bounded(&mut body, b"56", 6).expect("exact response limit should be accepted");
        let before = body.clone();

        let result = append_bounded(&mut body, b"7", 6);

        assert!(matches!(result, Err(BoundedResponseError::TooLarge)));
        assert_eq!(body, before);
    }

    #[test]
    fn equal_and_ahead_versions_do_not_report_an_update() {
        let package = asset("Squallz-v0.2.0-windows-x64.exe", None);
        let current = evaluate_release(
            release(vec![package.clone()]),
            "0.2.0",
            target("windows", "x64"),
            None,
            None,
        )
        .expect("equal version should resolve");
        let ahead = evaluate_release(
            release(vec![package]),
            "0.3.0",
            target("windows", "x64"),
            None,
            None,
        )
        .expect("ahead version should resolve");

        assert_eq!(current.status, UpdateStatus::UpToDate);
        assert_eq!(ahead.status, UpdateStatus::Ahead);
        assert_eq!(current.asset_name, None);
        assert_eq!(ahead.asset_name, None);
    }

    #[test]
    fn build_metadata_does_not_change_semver_precedence() {
        let result = evaluate_release(
            release(Vec::new()),
            "0.2.0+local.7",
            target("linux", "x64"),
            None,
            None,
        )
        .expect("build metadata is valid semantic versioning");

        assert_eq!(result.status, UpdateStatus::UpToDate);
    }

    #[test]
    fn unsupported_architecture_keeps_release_information_without_a_download() {
        let unsupported = ReleaseTarget {
            platform: "linux",
            architecture: "arm64",
            asset_platform: None,
            package: ReleasePackage::CommandLine,
        };
        let result = evaluate_release(release(Vec::new()), "0.1.0", unsupported, None, None)
            .expect("stable release should still be reported");

        assert_eq!(result.status, UpdateStatus::UpdateAvailable);
        assert_eq!(result.asset_name, None);
        assert_eq!(result.download_url, None);
        assert_eq!(result.asset_trust, UpdateTrust::Unavailable);
        assert_eq!(result.metadata_source, UpdateMetadataSource::GithubApi);
    }

    #[test]
    fn exact_latest_release_redirect_reports_version_without_package_claims() {
        let url = reqwest::Url::parse("https://github.com/yangzhg/Squallz/releases/tag/v0.2.0")
            .expect("test URL should parse");
        let release =
            release_from_latest_redirect(&url).expect("exact stable redirect should resolve");
        let result = evaluate_release_from_source(
            release,
            "0.1.0",
            target("macos", "arm64"),
            None,
            None,
            UpdateMetadataSource::LatestReleaseRedirect,
        )
        .expect("fallback release should resolve");

        assert_eq!(result.status, UpdateStatus::UpdateAvailable);
        assert_eq!(result.latest_version, "0.2.0");
        assert_eq!(
            result.release_url,
            "https://github.com/yangzhg/Squallz/releases/tag/v0.2.0"
        );
        assert_eq!(result.asset_name, None);
        assert_eq!(result.download_url, None);
        assert_eq!(result.asset_sha256, None);
        assert_eq!(result.asset_trust, UpdateTrust::Unavailable);
        assert_eq!(
            result.metadata_source,
            UpdateMetadataSource::LatestReleaseRedirect
        );
    }

    #[test]
    fn latest_release_manifest_recovers_exact_package_without_a_trust_claim() {
        let url = reqwest::Url::parse("https://github.com/yangzhg/Squallz/releases/tag/v0.2.0")
            .expect("test URL should parse");
        let mut release =
            release_from_latest_redirect(&url).expect("exact stable redirect should resolve");
        let digest = "b".repeat(64);
        let manifest = signed_manifest("Squallz-v0.2.0-macos-arm64.dmg", &digest);
        let asset = manifest_asset_for_target(&release, &manifest, target("macos", "arm64"))
            .expect("matching manifest should recover the package");
        release.assets.push(asset);

        let result = evaluate_release_from_source(
            release,
            "0.1.0",
            target("macos", "arm64"),
            Some(&manifest),
            None,
            UpdateMetadataSource::LatestReleaseManifest,
        )
        .expect("manifest fallback should resolve");

        assert_eq!(result.status, UpdateStatus::UpdateAvailable);
        assert_eq!(
            result.asset_name.as_deref(),
            Some("Squallz-v0.2.0-macos-arm64.dmg")
        );
        assert_eq!(
            result.download_url.as_deref(),
            Some("https://github.com/yangzhg/Squallz/releases/download/v0.2.0/Squallz-v0.2.0-macos-arm64.dmg")
        );
        assert_eq!(result.asset_size_bytes, Some(42));
        assert_eq!(result.asset_sha256.as_deref(), Some(digest.as_str()));
        assert_eq!(result.asset_trust, UpdateTrust::UnsignedPreview);
        assert_eq!(
            result.metadata_source,
            UpdateMetadataSource::LatestReleaseManifest
        );
    }

    #[test]
    fn latest_release_manifest_rejects_wrong_identity_duplicates_and_bad_digests() {
        let release = release(Vec::new());
        let package = "Squallz-v0.2.0-macos-arm64.dmg";
        let digest = "c".repeat(64);

        let mut wrong_repository = signed_manifest(package, &digest);
        wrong_repository.repository = "other/Squallz".to_owned();
        assert!(
            manifest_asset_for_target(&release, &wrong_repository, target("macos", "arm64"))
                .is_none()
        );

        let mut duplicate = signed_manifest(package, &digest);
        duplicate.assets.push(ReleaseManifestAsset {
            name: package.to_owned(),
            sha256: digest.clone(),
            size_bytes: 42,
            trust_state: Some("unsigned-preview".to_owned()),
        });
        assert!(
            manifest_asset_for_target(&release, &duplicate, target("macos", "arm64")).is_none()
        );

        let mut bad_digest = signed_manifest(package, &digest);
        bad_digest.assets[0].sha256 = "not-a-sha256".to_owned();
        assert!(
            manifest_asset_for_target(&release, &bad_digest, target("macos", "arm64")).is_none()
        );
    }

    #[test]
    fn duplicate_manifest_records_block_direct_download_and_trust() {
        let digest = "a".repeat(64);
        let package_name = "Squallz-v0.2.0-macos-arm64.dmg";
        let package = asset(package_name, Some(&format!("sha256:{digest}")));
        let mut manifest = signed_manifest(package_name, &digest);
        manifest.assets.push(ReleaseManifestAsset {
            name: package_name.to_owned(),
            sha256: digest.clone(),
            size_bytes: 42,
            trust_state: Some("developer-id-notarized".to_owned()),
        });
        let evidence = signed_trust_evidence(package_name, &digest);

        let result = evaluate_release(
            release(vec![package]),
            "0.1.0",
            target("macos", "arm64"),
            Some(&manifest),
            Some(&evidence),
        )
        .expect("the release version remains usable");

        assert_eq!(result.download_url, None);
        assert_eq!(result.asset_sha256, None);
        assert_eq!(result.asset_trust, UpdateTrust::Unavailable);
    }

    #[test]
    fn latest_release_redirect_rejects_untrusted_or_unstable_destinations() {
        let invalid_urls = [
            "http://github.com/yangzhg/Squallz/releases/tag/v0.2.0",
            "https://example.com/yangzhg/Squallz/releases/tag/v0.2.0",
            "https://github.com/other/Squallz/releases/tag/v0.2.0",
            "https://github.com/yangzhg/Squallz/releases/tag/v0.2.0/",
            "https://github.com/yangzhg/Squallz/releases/tag/v0.2.0-rc.1",
            "https://github.com/yangzhg/Squallz/releases/tag/v0.2.0?source=other",
            "https://github.com/yangzhg/Squallz/releases/tag/v0.2.0#packages",
        ];

        for value in invalid_urls {
            let url = reqwest::Url::parse(value).expect("test URL should parse");
            let error = release_from_latest_redirect(&url)
                .expect_err("non-canonical redirect must be rejected");
            assert_eq!(error.kind(), UpdateErrorKind::InvalidResponse);
        }
    }

    #[test]
    fn prerelease_and_foreign_urls_are_rejected() {
        let mut prerelease = release(Vec::new());
        prerelease.prerelease = true;
        let prerelease_error =
            evaluate_release(prerelease, "0.1.0", target("linux", "x64"), None, None)
                .expect_err("prerelease must be rejected");
        assert_eq!(prerelease_error.kind(), UpdateErrorKind::InvalidResponse);

        let mut foreign = release(Vec::new());
        foreign.html_url = "https://example.invalid/v0.2.0".to_owned();
        let foreign_error = evaluate_release(foreign, "0.1.0", target("linux", "x64"), None, None)
            .expect_err("foreign release URL must be rejected");
        assert_eq!(foreign_error.kind(), UpdateErrorKind::InvalidResponse);
    }

    #[test]
    fn prerelease_tag_is_rejected_even_when_github_marks_the_release_stable() {
        let mut prerelease = release(Vec::new());
        prerelease.tag_name = "v0.2.0-rc.1".to_owned();
        prerelease.html_url = format!("{RELEASE_BASE_URL}/tag/v0.2.0-rc.1");

        let error = evaluate_release(prerelease, "0.1.0", target("linux", "x64"), None, None)
            .expect_err("a prerelease tag must not enter the stable channel");

        assert_eq!(error.kind(), UpdateErrorKind::InvalidResponse);
    }

    #[test]
    fn release_manifest_supplies_a_missing_github_digest_for_unsigned_packages() {
        let digest = "e".repeat(64);
        let package_name = "Squallz-v0.2.0-macos-arm64.app.zip";
        let package = asset(package_name, None);
        let manifest = unsigned_manifest(package_name, &digest);

        let result = evaluate_release(
            release(vec![package]),
            "0.1.0",
            target("macos", "arm64"),
            Some(&manifest),
            None,
        )
        .expect("matching release metadata should resolve");

        assert_eq!(result.asset_sha256.as_deref(), Some(digest.as_str()));
        assert_eq!(result.asset_trust, UpdateTrust::UnsignedPreview);
    }

    #[test]
    fn conflicting_github_and_manifest_digests_are_not_displayed() {
        let github_digest = "f".repeat(64);
        let package_name = "Squallz-v0.2.0-windows-x64.exe";
        let package = asset(package_name, Some(&format!("sha256:{github_digest}")));
        let manifest = unsigned_manifest(package_name, &"0".repeat(64));

        let result = evaluate_release(
            release(vec![package]),
            "0.1.0",
            target("windows", "x64"),
            Some(&manifest),
            None,
        )
        .expect("the stable release remains visible");

        assert_eq!(result.asset_sha256, None);
        assert_eq!(result.download_url, None);
        assert_eq!(result.asset_trust, UpdateTrust::Unavailable);
    }

    #[test]
    fn mismatched_asset_url_and_invalid_digest_are_not_trusted() {
        let mut package = asset(
            "Squallz-v0.2.0-linux-x64.tar.gz",
            Some("sha256:not-a-digest"),
        );
        package.browser_download_url =
            "https://example.invalid/Squallz-v0.2.0-linux-x64.tar.gz".to_owned();
        let result = evaluate_release(
            release(vec![package]),
            "0.1.0",
            target("linux", "x64"),
            None,
            None,
        )
        .expect("release itself remains valid");

        assert_eq!(result.asset_name, None);
        assert_eq!(result.asset_sha256, None);
        assert_eq!(result.asset_trust, UpdateTrust::Unavailable);
    }

    #[test]
    fn malformed_github_digest_blocks_the_direct_download() {
        let package = asset(
            "Squallz-v0.2.0-linux-x64.tar.gz",
            Some("sha256:not-a-digest"),
        );
        let result = evaluate_release(
            release(vec![package]),
            "0.1.0",
            target("linux", "x64"),
            None,
            None,
        )
        .expect("the stable release itself remains visible");

        assert_eq!(
            result.asset_name.as_deref(),
            Some("Squallz-v0.2.0-linux-x64.tar.gz")
        );
        assert_eq!(result.download_url, None);
        assert_eq!(result.asset_sha256, None);
        assert_eq!(result.asset_trust, UpdateTrust::Unavailable);
    }

    #[test]
    fn mismatched_manifest_blocks_the_direct_download() {
        let digest = "b".repeat(64);
        let package_name = "Squallz-v0.2.0-macos-arm64.dmg";
        let package = asset(package_name, Some(&format!("sha256:{digest}")));
        let trust = asset(&format!("{package_name}.trust.json"), None);
        let wrong_manifest = signed_manifest(package_name, &"c".repeat(64));
        let result = evaluate_release(
            release(vec![package, trust]),
            "0.1.0",
            target("macos", "arm64"),
            Some(&wrong_manifest),
            None,
        )
        .expect("release itself remains valid");

        assert_eq!(result.download_url, None);
        assert_eq!(result.asset_sha256, None);
        assert_eq!(result.asset_trust, UpdateTrust::Unavailable);
    }

    #[test]
    fn malformed_trust_evidence_stays_unsigned() {
        let digest = "d".repeat(64);
        let package_name = "Squallz-v0.2.0-macos-arm64.dmg";
        let package = asset(package_name, Some(&format!("sha256:{digest}")));
        let trust = asset(&format!("{package_name}.trust.json"), None);
        let manifest = signed_manifest(package_name, &digest);
        let mut evidence = signed_trust_evidence(package_name, &digest);
        evidence.gatekeeper = false;

        let result = evaluate_release(
            release(vec![package, trust]),
            "0.1.0",
            target("macos", "arm64"),
            Some(&manifest),
            Some(&evidence),
        )
        .expect("release itself remains valid");

        assert_eq!(result.asset_trust, UpdateTrust::UnsignedPreview);
    }

    #[test]
    fn notarized_original_dmg_name_survives_public_release_rename() {
        let digest = "d".repeat(64);
        let package_name = "Squallz-v0.2.0-macos-arm64.dmg";
        let package = asset(package_name, Some(&format!("sha256:{digest}")));
        let manifest = signed_manifest(package_name, &digest);
        let evidence = signed_trust_evidence("Squallz_0.2.0_aarch64.dmg", &digest);

        let result = evaluate_release(
            release(vec![package]),
            "0.1.0",
            target("macos", "arm64"),
            Some(&manifest),
            Some(&evidence),
        )
        .expect("renaming after notarization must preserve byte-bound evidence");

        assert_eq!(result.asset_trust, UpdateTrust::DeveloperIdNotarized);
    }

    #[test]
    fn invalid_installed_version_is_rejected_before_network_setup() {
        let update = normalize_current_version("not-a-version")
            .expect_err("an invalid installed version must be rejected");

        assert_eq!(update.kind(), UpdateErrorKind::InvalidResponse);
        assert_eq!(update.i18n_key(), "error.update.invalid_response");
    }

    #[test]
    fn notarization_evidence_is_required_only_for_macos_dmg_packages() {
        let dmg = asset("Squallz-v0.2.0-macos-arm64.dmg", None);
        let app_zip = asset("Squallz-v0.2.0-macos-arm64.app.zip", None);
        let windows = asset("Squallz-v0.2.0-windows-x64.exe", None);

        assert!(requires_release_trust_evidence(
            target("macos", "arm64"),
            Some(&dmg)
        ));
        assert!(!requires_release_trust_evidence(
            target("macos", "arm64"),
            Some(&app_zip)
        ));
        assert!(!requires_release_trust_evidence(
            target("windows", "x64"),
            Some(&windows)
        ));
        assert!(!requires_release_trust_evidence(
            target("macos", "arm64"),
            None
        ));
    }
}
