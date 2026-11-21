use super::{Options, Selection};
use anyhow::{ensure, Context, Result};
use ring::{digest, signature};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
};

pub(super) const MAX_PACKAGE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_EXTRACTED_BYTES: u64 = 4 * 1024 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BundleLayout {
    pub schema: u8,
    pub entrypoint: String,
    pub entries: Vec<BundleEntry>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub struct BundleEntry {
    pub path: String,
    pub kind: EntryKind,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum EntryKind {
    File,
    Directory,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PackageFile {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

/// One delta artifact carries only the files that changed from `from`.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Delta {
    pub from: String,
    pub artifact: String,
    pub size: u64,
    pub sha256: String,
    pub files: Vec<PackageFile>,
}

#[derive(Clone, Deserialize, Serialize)]
pub(super) struct Package {
    pub artifact: String,
    pub size: u64,
    pub sha256: String,
    #[serde(default)]
    pub files: Vec<PackageFile>,
    #[serde(default)]
    pub deltas: Vec<Delta>,
}

#[derive(Deserialize)]
struct Index {
    schema: u8,
    version: String,
    revision: String,
    targets: BTreeMap<String, Package>,
}

#[derive(Clone)]
pub(super) struct Installation {
    pub root: PathBuf,
    pub data_root: PathBuf,
    pub working_directory: PathBuf,
}

impl Installation {
    pub fn inspect(options: &Options, config: &super::super::Config) -> Result<Self> {
        let directory = options
            .executable
            .parent()
            .context("App directory is missing")?;
        ensure!(
            entry_kind(&fs::symlink_metadata(directory)?)? == EntryKind::Directory,
            "App directory must be a real directory"
        );
        let root = fs::canonicalize(directory)?;
        options.layout.check(&root)?;
        ensure!(
            options
                .executable
                .file_name()
                .and_then(|name| name.to_str())
                == Some(&options.layout.entrypoint),
            "App entrypoint does not match this package"
        );
        let data_root = fs::canonicalize(&config.data_root)?;
        ensure!(
            outside(&root, &data_root),
            "App data must remain outside the program directory"
        );
        ensure!(
            same_path(&fs::canonicalize(&config.assets)?, &root.join("web"))
                && same_path(
                    &fs::canonicalize(&config.cloudflared)?,
                    &root.join("runtime/tunnel/cloudflared.exe")
                ),
            "Custom App resources require a manual update"
        );
        ensure!(
            options.target == "windows-amd64",
            "This package target is not supported for installation"
        );
        Ok(Self {
            root,
            data_root,
            working_directory: fs::canonicalize(&options.working_directory)?,
        })
    }
}

pub(super) struct Prepared {
    pub directory: tempfile::TempDir,
    pub selection: Selection,
    pub operation: uuid::Uuid,
    pub delta: Option<Delta>,
}

pub(super) fn hex<const N: usize>(value: &str) -> Option<[u8; N]> {
    if value.len() != N * 2 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let mut result = [0; N];
    for (index, byte) in result.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(result)
}

pub(super) fn validate_selection(current: &str, selection: &Selection) -> Result<()> {
    let version = semver::Version::parse(
        selection
            .version
            .strip_prefix('v')
            .context("Invalid version")?,
    )?;
    let current = semver::Version::parse(
        current
            .strip_prefix('v')
            .context("Unknown installed version")?,
    )?;
    ensure!(
        selection.version == format!("v{version}")
            && version.pre.is_empty()
            && version.build.is_empty()
            && version.cmp_precedence(&current).is_gt(),
        "An update must be a newer stable release"
    );
    ensure!(
        hex::<20>(&selection.revision).is_some()
            && selection.revision == selection.revision.to_ascii_lowercase(),
        "Invalid source revision"
    );
    Ok(())
}

pub(super) fn verify_index(
    key: &[u8; 32],
    bytes: &[u8],
    signature: &[u8],
    selection: &Selection,
    target: &str,
) -> Result<Package> {
    signature::UnparsedPublicKey::new(&signature::ED25519, key)
        .verify(bytes, signature)
        .map_err(|_| anyhow::anyhow!("Update signature is invalid"))?;
    let mut index: Index = serde_json::from_slice(bytes)?;
    ensure!(
        index.schema == 1
            && index.version == selection.version
            && index.revision == selection.revision,
        "Signed release identity does not match the selected release"
    );
    let package = index
        .targets
        .remove(target)
        .context("Signed release has no matching App package")?;
    ensure!(
        package.artifact == format!("piik-app-{target}.zip")
            && package.size > 0
            && package.size <= MAX_PACKAGE_BYTES
            && hex::<32>(&package.sha256).is_some()
            && package.sha256 == package.sha256.to_ascii_lowercase(),
        "Invalid signed App package"
    );
    validate_package_files(&package)?;
    validate_deltas(&package, target, &selection.version)?;
    Ok(package)
}

/// A delta may only carry files that the full signed manifest already binds,
/// and its `from` version must be an older stable release of this target.
fn validate_deltas(package: &Package, target: &str, release: &str) -> Result<()> {
    ensure!(
        package.deltas.len() <= 8,
        "Signed App package has too many deltas"
    );
    let release_version = semver::Version::parse(release.strip_prefix('v').context("Invalid version")?)?;
    let full: BTreeMap<String, &PackageFile> = package
        .files
        .iter()
        .map(|file| (file.path.to_ascii_lowercase(), file))
        .collect();
    let mut versions = BTreeSet::new();
    let mut artifacts = BTreeSet::new();
    for delta in &package.deltas {
        let version = semver::Version::parse(
            delta
                .from
                .strip_prefix('v')
                .context("Invalid delta version")?,
        )?;
        ensure!(
            delta.from == format!("v{version}")
                && version.pre.is_empty()
                && version.build.is_empty()
                && version.cmp_precedence(&release_version).is_lt()
                && versions.insert(delta.from.clone()),
            "Invalid delta source version"
        );
        ensure!(
            delta.artifact == format!("piik-app-{target}-delta-{}.zip", delta.from)
                && artifacts.insert(delta.artifact.clone())
                && delta.size > 0
                && delta.size <= MAX_PACKAGE_BYTES
                && hex::<32>(&delta.sha256).is_some()
                && delta.sha256 == delta.sha256.to_ascii_lowercase(),
            "Invalid signed App delta"
        );
        ensure!(!delta.files.is_empty(), "Signed App delta has no files");
        let mut paths = BTreeSet::new();
        for file in &delta.files {
            let Some(expected) = full.get(&file.path.to_ascii_lowercase()) else {
                anyhow::bail!("Delta contains a file outside the release manifest");
            };
            ensure!(
                paths.insert(file.path.to_ascii_lowercase())
                    && file.path == expected.path
                    && file.size == expected.size
                    && file.sha256 == expected.sha256,
                "Delta file does not match the release manifest"
            );
        }
    }
    Ok(())
}

/// The signed file manifest binds every extracted path to its exact bytes, so a
/// package-level digest alone cannot hide a changed or unexpected entry.
fn validate_package_files(package: &Package) -> Result<()> {
    ensure!(
        !package.files.is_empty(),
        "Signed App package has no file manifest"
    );
    let mut paths = BTreeSet::new();
    let mut total = 0u64;
    for file in &package.files {
        ensure!(
            relative_path(&file.path)
                && file.size > 0
                && hex::<32>(&file.sha256).is_some()
                && file.sha256 == file.sha256.to_ascii_lowercase()
                && paths.insert(file.path.to_ascii_lowercase()),
            "Invalid signed App file manifest"
        );
        total = total
            .checked_add(file.size)
            .context("App file manifest is too large")?;
    }
    ensure!(
        total <= MAX_EXTRACTED_BYTES,
        "App file manifest is too large"
    );
    for required in [
        "piik-app.exe",
        "web/index.html",
        "runtime/tunnel/cloudflared.exe",
        "REVISION",
    ] {
        ensure!(
            paths.contains(&required.to_ascii_lowercase()),
            "App file manifest is incomplete"
        );
    }
    Ok(())
}

/// Verify every extracted file against the signed manifest. Extra files are as
/// fatal as changed ones: the installed directory must be exactly the release.
pub(super) fn verify_files(root: &Path, files: &[PackageFile]) -> Result<()> {
    fn walk(
        root: &Path,
        directory: &Path,
        expected: &BTreeMap<String, &PackageFile>,
        found: &mut BTreeSet<String>,
    ) -> Result<()> {
        for entry in fs::read_dir(directory)? {
            let path = entry?.path();
            let kind = entry_kind(&fs::symlink_metadata(&path)?)?;
            let relative = path
                .strip_prefix(root)?
                .components()
                .map(|component| {
                    component
                        .as_os_str()
                        .to_str()
                        .context("Non-Unicode App path")
                })
                .collect::<Result<Vec<_>>>()?
                .join("/");
            if kind == EntryKind::Directory {
                walk(root, &path, expected, found)?;
                continue;
            }
            let key = relative.to_ascii_lowercase();
            let file = expected
                .get(&key)
                .with_context(|| format!("Unexpected App file: {relative}"))?;
            ensure!(found.insert(key), "Duplicate App file: {relative}");
            verify_file(&path, file, &relative)?;
        }
        Ok(())
    }
    let mut expected = BTreeMap::new();
    for file in files {
        expected.insert(file.path.to_ascii_lowercase(), file);
    }
    let mut found = BTreeSet::new();
    walk(root, root, &expected, &mut found)?;
    ensure!(
        found.len() == expected.len(),
        "App file manifest is incomplete"
    );
    Ok(())
}

fn verify_file(path: &Path, file: &PackageFile, label: &str) -> Result<()> {
    ensure!(
        fs::metadata(path)?.len() == file.size,
        "App file size changed: {label}"
    );
    let mut source = File::open(path)?;
    let mut hash = digest::Context::new(&digest::SHA256);
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = source.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    ensure!(
        hash.finish().as_ref() == hex::<32>(&file.sha256).context("Invalid App file digest")?,
        "App file digest changed: {label}"
    );
    Ok(())
}

fn delta_changed(delta: &Delta) -> BTreeMap<String, &PackageFile> {
    delta
        .files
        .iter()
        .map(|file| (file.path.to_ascii_lowercase(), file))
        .collect()
}

/// True when every file the delta does not replace still matches the release.
pub(super) fn delta_is_applicable(current: &Path, package: &Package, delta: &Delta) -> bool {
    let changed = delta_changed(delta);
    package
        .files
        .iter()
        .filter(|file| !changed.contains_key(&file.path.to_ascii_lowercase()))
        .all(|file| verify_file(&current.join(&file.path), file, &file.path).is_ok())
}

/// Pick the delta that upgrades this installation, if one is published and the
/// installed files still match every file that delta does not replace.
pub(super) fn select_delta(
    package: &Package,
    options: &Options,
    installation: &Installation,
) -> Option<Delta> {
    package
        .deltas
        .iter()
        .find(|delta| {
            delta.from == options.version && delta_is_applicable(&installation.root, package, delta)
        })
        .cloned()
}

/// Verify an extracted delta stage against its signed records.
pub(super) fn verify_delta_files(root: &Path, delta: &Delta, package: &Package) -> Result<()> {
    let changed = delta_changed(delta);
    ensure!(
        changed.len() == delta.files.len(),
        "Signed App delta has duplicate files"
    );
    for file in &delta.files {
        let expected = package
            .files
            .iter()
            .find(|candidate| candidate.path == file.path)
            .context("Delta file is outside the release manifest")?;
        ensure!(
            expected.size == file.size && expected.sha256 == file.sha256,
            "Delta file does not match the release manifest"
        );
    }
    verify_files(root, &delta.files)
}

/// Build the complete incoming tree: unchanged files come from the installed
/// program directory, changed files from the verified delta stage.
pub(super) fn assemble_delta(
    current: &Path,
    stage: &Path,
    package: &Package,
    delta: &Delta,
    incoming: &Path,
) -> Result<()> {
    let changed = delta_changed(delta);
    for file in &package.files {
        let key = file.path.to_ascii_lowercase();
        let source = if changed.contains_key(&key) {
            stage.join(&file.path)
        } else {
            current.join(&file.path)
        };
        let destination = incoming.join(&file.path);
        fs::create_dir_all(destination.parent().context("App file has no parent")?)?;
        fs::copy(&source, &destination).with_context(|| format!("Copy {}", file.path))?;
    }
    verify_files(incoming, &package.files)
}

pub(super) fn verify_archive(path: &Path, package: &Package) -> Result<()> {
    verify_artifact(path, package.size, &package.sha256)
}

pub(super) fn verify_delta_archive(path: &Path, delta: &Delta) -> Result<()> {
    verify_artifact(path, delta.size, &delta.sha256)
}

fn verify_artifact(path: &Path, size: u64, sha256: &str) -> Result<()> {
    ensure!(
        fs::metadata(path)?.len() == size,
        "Update package size changed"
    );
    let mut source = File::open(path)?;
    let mut hash = digest::Context::new(&digest::SHA256);
    let mut bytes = [0; 64 * 1024];
    loop {
        let count = source.read(&mut bytes)?;
        if count == 0 {
            break;
        }
        hash.update(&bytes[..count]);
    }
    ensure!(
        hash.finish().as_ref() == hex::<32>(sha256).context("Invalid package digest")?,
        "Update package digest changed"
    );
    Ok(())
}

fn relative_path(path: &str) -> bool {
    !path.is_empty()
        && !path.contains(['\\', ':', '\0', '<', '>', '"', '|', '?', '*'])
        && path.split('/').all(|part| {
            let stem = part.split('.').next().unwrap_or("").to_ascii_uppercase();
            !part.is_empty()
                && !part.ends_with(['.', ' '])
                && !matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
                && !(stem.len() == 4
                    && (stem.starts_with("COM") || stem.starts_with("LPT"))
                    && matches!(stem.as_bytes()[3], b'1'..=b'9'))
        })
}

pub(super) fn entry_kind(metadata: &fs::Metadata) -> Result<EntryKind> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        ensure!(
            metadata.file_attributes() & 0x400 == 0,
            "Update directories cannot contain reparse points"
        );
    }
    ensure!(
        !metadata.file_type().is_symlink(),
        "Update directories cannot contain links"
    );
    if metadata.is_file() {
        Ok(EntryKind::File)
    } else if metadata.is_dir() {
        Ok(EntryKind::Directory)
    } else {
        anyhow::bail!("Unsupported update directory entry")
    }
}

pub(super) fn entries(root: &Path) -> Result<Vec<BundleEntry>> {
    fn walk(root: &Path, directory: &Path, found: &mut Vec<BundleEntry>) -> Result<()> {
        for entry in fs::read_dir(directory)? {
            let path = entry?.path();
            let kind = entry_kind(&fs::symlink_metadata(&path)?)?;
            let relative = path
                .strip_prefix(root)?
                .components()
                .map(|component| {
                    component
                        .as_os_str()
                        .to_str()
                        .context("Non-Unicode package path")
                })
                .collect::<Result<Vec<_>>>()?
                .join("/");
            found.push(BundleEntry {
                path: relative,
                kind,
            });
            ensure!(found.len() <= 100_000, "Too many package entries");
            if kind == EntryKind::Directory {
                walk(root, &path, found)?;
            }
        }
        Ok(())
    }
    ensure!(
        entry_kind(&fs::symlink_metadata(root)?)? == EntryKind::Directory,
        "Expected a real directory"
    );
    let mut result = vec![];
    walk(root, root, &mut result)?;
    result.sort();
    Ok(result)
}

impl BundleLayout {
    pub(super) fn check(&self, root: &Path) -> Result<()> {
        ensure!(
            self.schema == 1 && relative_path(&self.entrypoint) && !self.entrypoint.contains('/'),
            "Invalid package layout"
        );
        let mut expected = self.entries.clone();
        expected.sort();
        let mut unique = BTreeSet::new();
        for entry in &expected {
            ensure!(
                relative_path(&entry.path) && unique.insert(entry.path.to_ascii_lowercase()),
                "Invalid package layout entry"
            );
        }
        ensure!(
            expected.contains(&BundleEntry {
                path: self.entrypoint.clone(),
                kind: EntryKind::File
            }),
            "Missing package entrypoint"
        );
        ensure!(
            entries(root)? == expected,
            "Program directory contains missing, changed or additional entries"
        );
        Ok(())
    }
}

pub(super) fn same_path(a: &Path, b: &Path) -> bool {
    #[cfg(windows)]
    {
        a.as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case(&b.as_os_str().to_string_lossy())
    }
    #[cfg(not(windows))]
    {
        a == b
    }
}

pub(super) fn outside(root: &Path, path: &Path) -> bool {
    !path.ancestors().any(|parent| same_path(root, parent))
}

pub(super) fn extract(archive: &Path, stage: &Path, entrypoint: &str) -> Result<()> {
    extract_archive(archive, stage)?;
    ensure!(
        stage.join(entrypoint).is_file()
            && stage.join("web/index.html").is_file()
            && stage.join("runtime/tunnel/cloudflared.exe").is_file()
            && stage.join("REVISION").is_file(),
        "Update package is incomplete"
    );
    Ok(())
}

/// Delta archives carry only changed files; the full manifest is verified after
/// they are assembled over the installed program directory.
pub(super) fn extract_delta(archive: &Path, stage: &Path) -> Result<()> {
    extract_archive(archive, stage)
}

fn extract_archive(archive: &Path, stage: &Path) -> Result<()> {
    fs::create_dir(stage)?;
    let mut archive = zip::ZipArchive::new(File::open(archive)?)?;
    ensure!(archive.len() <= 100_000, "Too many update entries");
    let mut paths = BTreeSet::new();
    let mut expanded = 0u64;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let name = entry.name()?.trim_end_matches('/').to_owned();
        ensure!(
            relative_path(&name)
                && entry.enclosed_name().is_some()
                && paths.insert(name.to_ascii_lowercase()),
            "Invalid or duplicate update path"
        );
        let mode = entry.unix_mode().unwrap_or(0) & 0o170000;
        ensure!(
            mode == 0
                || (entry.is_dir() && mode == 0o040000)
                || (entry.is_file() && mode == 0o100000),
            "Update packages may contain only regular files and directories"
        );
        let destination = stage.join(&name);
        if entry.is_dir() {
            fs::create_dir_all(destination)?;
            continue;
        }
        ensure!(entry.is_file(), "Invalid update entry");
        expanded = expanded
            .checked_add(entry.size())
            .context("Update package is too large")?;
        ensure!(
            expanded <= MAX_EXTRACTED_BYTES,
            "Update package is too large"
        );
        fs::create_dir_all(destination.parent().unwrap())?;
        let mut output = File::options()
            .write(true)
            .create_new(true)
            .open(destination)?;
        let size = entry.size();
        let copied = std::io::copy(&mut (&mut entry).take(size + 1), &mut output)?;
        ensure!(copied == size, "Update entry size mismatch");
        output.flush()?;
    }
    Ok(())
}
