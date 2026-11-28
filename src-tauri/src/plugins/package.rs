//! Plugin packages — one plugin folder as a portable `.zip`.
//!
//! Export writes `plugin.json` and the payload (entry script, schema, assets) at
//! the archive root, so unzipping a package by hand yields exactly the folder
//! `<workspace>/.data/plugins/<id>/` expects. Values in `config.yaml` that the
//! schema marks `secret` are stripped before packing; a plugin that declares no
//! schema keeps its settings out of the archive altogether, because an opaque
//! values file may hold a token nobody declared.
//!
//! Import accepts that shape and, for an archive zipped by hand, a single
//! top-level folder holding `plugin.json`. Extraction is confined to the plugin
//! folder (zip-slip protected), the copy a re-import replaces is moved to
//! `<workspace>/.data/plugin-backups/<id>-<stamp>/` instead of being deleted,
//! and every failure after the first write rolls the folder back.

use super::{
    config, load_plugins, plugin_view, plugins_dir, slugify_segment, Plugin, PluginManifest,
    PluginView, MAX_PLUGINS,
};
use serde::Serialize;
use std::fs;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use zip::write::FileOptions;
use zip::{ZipArchive, ZipWriter};

/// Entry file every plugin folder carries.
pub const MANIFEST_FILE: &str = "plugin.json";
/// Folder under `<workspace>/.data/` holding the copy an import replaced.
pub const BACKUP_DIR_NAME: &str = "plugin-backups";
/// Ceiling on the files one package may carry (packed and unpacked).
pub const MAX_PACKAGE_FILES: usize = 4096;
/// Ceiling on the unpacked size of one package, in bytes (zip-bomb guard).
pub const MAX_PACKAGE_BYTES: u64 = 128 * 1024 * 1024;

/// Directories that are dependency or build noise, never plugin payload.
const SKIP_DIRS: [&str; 6] = [
    "node_modules",
    "__pycache__",
    ".git",
    ".venv",
    ".mypy_cache",
    ".pytest_cache",
];

// ─── Reports (wire shapes for `src/api.ts`) ──────────────────────────────────

/// What one export put into the archive.
#[derive(Debug, Clone, Serialize)]
pub struct PluginExportReport {
    pub id: String,
    pub path: String,
    /// Files written, the manifest included.
    pub files: usize,
    /// Size of the archive on disk (compressed).
    pub bytes: u64,
    /// Files left out: hidden ones, editor backups, `config.yaml` without a schema.
    pub skipped: usize,
    /// Secret values removed from the packed `config.yaml`.
    pub secrets_removed: usize,
    /// Whether the archive carries settings at all.
    pub settings_exported: bool,
}

/// What one import installed.
#[derive(Debug, Clone, Serialize)]
pub struct PluginImportReport {
    pub plugin: PluginView,
    pub dir: String,
    pub files: usize,
    pub bytes: u64,
    /// Where the copy this import replaced was moved; `None` for a fresh install.
    pub replaced: Option<String>,
}

// ─── Paths and small helpers ─────────────────────────────────────────────────

/// Where a replaced plugin folder is kept (`<workspace>/.data/plugin-backups`).
///
/// Deliberately outside `plugins/`: the loader reads every folder that holds a
/// `plugin.json`, so a copy parked next door would load as a second plugin.
pub fn backup_root(workspace: &Path) -> PathBuf {
    crate::workspace::data_root(workspace).join(BACKUP_DIR_NAME)
}

fn zip_options() -> FileOptions {
    FileOptions::default().compression_method(zip::CompressionMethod::Deflated)
}

/// Resolve `relative` under `root`, rejecting absolute paths, drive prefixes and
/// `..` components (zip-slip protection). `None` when nothing is left.
fn safe_join(root: &Path, relative: &str) -> Option<PathBuf> {
    let mut target = root.to_path_buf();
    let mut has_component = false;
    for component in Path::new(relative).components() {
        match component {
            Component::Normal(part) => {
                target.push(part);
                has_component = true;
            }
            Component::CurDir => {}
            Component::Prefix(_) | Component::RootDir | Component::ParentDir => return None,
        }
    }
    has_component.then_some(target)
}

/// Hidden entries and editor backups ride along in a folder; they never pack.
fn is_noise(name: &str) -> bool {
    name.starts_with('.') || name.contains(".bak.")
}

/// Every file under `dir`, minus dependencies, caches and noise.
fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = fs::read_dir(dir).map_err(|err| format!("read {}: {err}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|err| format!("read {}: {err}", dir.display()))?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        let meta = fs::symlink_metadata(&path)
            .map_err(|err| format!("stat {}: {err}", path.display()))?;
        // A symlink can point outside the plugin; packing it would smuggle that in.
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            if is_noise(&name) || SKIP_DIRS.iter().any(|skip| name.eq_ignore_ascii_case(skip)) {
                continue;
            }
            collect_files(&path, out)?;
        } else if meta.is_file() && !is_noise(&name) {
            out.push(path);
        }
    }
    Ok(())
}

/// `config.yaml` with the schema's secrets removed, ready to pack.
///
/// `Ok((None, ..))` means "keep the settings at home": no schema to tell a
/// secret from a preference, or nothing but secrets left after stripping.
fn exportable_settings(plugin: &Plugin) -> Result<(Option<String>, usize), String> {
    let Some(schema) = config::load_schema(&plugin.dir, plugin.manifest.schema_file())? else {
        return Ok((None, 0));
    };
    let mut values = config::load_values(&plugin.dir);
    if values.is_empty() {
        return Ok((None, 0));
    }
    let secrets: Vec<String> = config::fields(&schema)
        .iter()
        .filter(|field| field.kind == config::FieldKind::Secret)
        .map(|field| field.key.clone())
        .collect();
    let mut removed = 0usize;
    for key in secrets {
        if values.remove(&key).is_some() {
            removed += 1;
        }
    }
    if values.is_empty() {
        return Ok((None, removed));
    }
    let text = serde_yaml::to_string(&serde_json::Value::Object(values))
        .map_err(|err| format!("serialize settings: {err}"))?;
    Ok((Some(text), removed))
}

fn backup_path_for(workspace: &Path, id: &str) -> PathBuf {
    let root = backup_root(workspace);
    let stamp = crate::util::now_ms_u64();
    let mut path = root.join(format!("{id}-{stamp}"));
    let mut suffix = 2u32;
    while path.exists() {
        path = root.join(format!("{id}-{stamp}-{suffix}"));
        suffix += 1;
    }
    path
}

// ─── Export ─────────────────────────────────────────────────────────────────

/// Pack one installed plugin into `dest` (a `.zip` path).
pub fn export_plugin(
    workspace: &Path,
    id: &str,
    dest: &Path,
) -> Result<PluginExportReport, String> {
    let registry = load_plugins(workspace);
    let plugin = registry
        .get(id)
        .ok_or_else(|| format!("plugin '{id}' is not installed"))?;

    let manifest_path = plugin.dir.join(MANIFEST_FILE);
    let manifest_text = fs::read_to_string(&manifest_path)
        .map_err(|err| format!("read {}: {err}", manifest_path.display()))?;
    let (settings, secrets_removed) = exportable_settings(plugin)?;

    let mut paths = Vec::new();
    collect_files(&plugin.dir, &mut paths)?;
    let mut payload: Vec<(String, PathBuf)> = Vec::new();
    let mut skipped = 0usize;
    let mut unpacked = manifest_text.len() as u64
        + settings.as_ref().map(|text| text.len() as u64).unwrap_or(0);
    for path in paths {
        let relative = path
            .strip_prefix(&plugin.dir)
            .map_err(|_| format!("{} is not inside the plugin folder", path.display()))?;
        let name = relative.to_string_lossy().replace('\\', "/");
        if name == MANIFEST_FILE {
            // Already written from the text read above; a second entry would
            // make `locate_manifest` see two plugins in one archive.
            continue;
        }
        if name == config::VALUES_FILE {
            // Replaced by the stripped copy below, or left behind when the
            // plugin has no schema to separate secrets from preferences.
            if settings.is_none() {
                skipped += 1;
            }
            continue;
        }
        let size = fs::metadata(&path)
            .map_err(|err| format!("stat {}: {err}", path.display()))?
            .len();
        unpacked += size;
        payload.push((name, path));
    }

    let files = payload.len() + 1 + usize::from(settings.is_some());
    if files > MAX_PACKAGE_FILES {
        return Err(format!(
            "the plugin holds {files} files; a package carries at most {MAX_PACKAGE_FILES}"
        ));
    }
    if unpacked > MAX_PACKAGE_BYTES {
        return Err(format!(
            "the plugin is {:.1} MB unpacked; a package carries at most {} MB — trim the folder and export again",
            unpacked as f64 / (1024.0 * 1024.0),
            MAX_PACKAGE_BYTES / (1024 * 1024)
        ));
    }

    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|err| format!("create {}: {err}", parent.display()))?;
    }
    let file = fs::File::create(dest).map_err(|err| format!("create {}: {err}", dest.display()))?;
    let mut zip = ZipWriter::new(file);

    zip.start_file(MANIFEST_FILE, zip_options())
        .map_err(|err| format!("write {MANIFEST_FILE}: {err}"))?;
    zip.write_all(manifest_text.as_bytes())
        .map_err(|err| format!("write {MANIFEST_FILE}: {err}"))?;

    if let Some(text) = &settings {
        zip.start_file(config::VALUES_FILE, zip_options())
            .map_err(|err| format!("write {}: {err}", config::VALUES_FILE))?;
        zip.write_all(text.as_bytes())
            .map_err(|err| format!("write {}: {err}", config::VALUES_FILE))?;
    }

    for (name, path) in &payload {
        zip.start_file(name.as_str(), zip_options())
            .map_err(|err| format!("write {name}: {err}"))?;
        let mut source =
            fs::File::open(path).map_err(|err| format!("read {}: {err}", path.display()))?;
        std::io::copy(&mut source, &mut zip).map_err(|err| format!("write {name}: {err}"))?;
    }

    zip.finish()
        .map_err(|err| format!("close {}: {err}", dest.display()))?;
    let bytes = fs::metadata(dest).map(|meta| meta.len()).unwrap_or(0);

    Ok(PluginExportReport {
        id: plugin.id().to_string(),
        path: dest.to_string_lossy().to_string(),
        files,
        bytes,
        skipped,
        secrets_removed,
        settings_exported: settings.is_some(),
    })
}

// ─── Import ─────────────────────────────────────────────────────────────────

/// The folder inside the archive that holds the manifest, plus the entry name.
///
/// A package exported here keeps `plugin.json` at the root; an archive zipped by
/// hand usually wraps the folder. Two manifests at the same depth make the
/// archive ambiguous, so it is refused rather than guessed.
fn locate_manifest(entries: &[(String, u64, usize)]) -> Result<(String, String), String> {
    let mut manifests: Vec<&str> = entries
        .iter()
        .map(|(name, _, _)| name.as_str())
        .filter(|name| name.rsplit('/').next() == Some(MANIFEST_FILE))
        .collect();
    if manifests.is_empty() {
        return Err(format!(
            "the archive holds no {MANIFEST_FILE} at its root or inside one folder"
        ));
    }
    manifests.sort_by_key(|name| name.matches('/').count());
    let depth = manifests[0].matches('/').count();
    let shallowest: Vec<&str> = manifests
        .into_iter()
        .filter(|name| name.matches('/').count() == depth)
        .collect();
    if shallowest.len() > 1 {
        return Err(format!(
            "the archive holds {} plugins; import one package at a time",
            shallowest.len()
        ));
    }
    let manifest = shallowest[0].to_string();
    let prefix = match manifest.rfind('/') {
        Some(at) => manifest[..at].to_string(),
        None => String::new(),
    };
    Ok((prefix, manifest))
}

/// Entry name relative to the plugin folder; `None` when it sits beside it.
fn strip_prefix(name: &str, prefix: &str) -> Option<String> {
    if prefix.is_empty() {
        return Some(name.to_string());
    }
    let rest = name.strip_prefix(prefix)?;
    Some(rest.strip_prefix('/')?.to_string())
}

/// Write the archive's payload into `staging`; returns `(files, bytes)`.
fn extract(
    archive: &mut ZipArchive<fs::File>,
    entries: &[(String, u64, usize)],
    prefix: &str,
    staging: &Path,
) -> Result<(usize, u64), String> {
    let mut files = 0usize;
    let mut bytes = 0u64;
    for (name, _, index) in entries {
        let Some(relative) = strip_prefix(name, prefix) else {
            continue;
        };
        if relative.is_empty() {
            continue;
        }
        let parts: Vec<&str> = relative.split('/').collect();
        if parts.iter().any(|part| part.is_empty() || is_noise(part))
            || parts.first() == Some(&"__MACOSX")
        {
            continue;
        }
        let target = safe_join(staging, &relative)
            .ok_or_else(|| format!("the archive entry '{name}' escapes the plugin folder"))?;
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .map_err(|err| format!("create {}: {err}", parent.display()))?;
        }
        let mut entry = archive
            .by_index(*index)
            .map_err(|err| format!("read entry '{name}': {err}"))?;
        let mut out = fs::File::create(&target)
            .map_err(|err| format!("create {}: {err}", target.display()))?;
        bytes += std::io::copy(&mut entry, &mut out)
            .map_err(|err| format!("unpack '{name}': {err}"))?;
        if bytes > MAX_PACKAGE_BYTES {
            return Err(format!(
                "the archive unpacks to more than {} MB",
                MAX_PACKAGE_BYTES / (1024 * 1024)
            ));
        }
        files += 1;
        // A shell or `exec` entry keeps the bit it was packed with.
        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            if mode & 0o111 != 0 {
                let _ = fs::set_permissions(&target, fs::Permissions::from_mode(mode & 0o777));
            }
        }
    }
    if !staging.join(MANIFEST_FILE).is_file() {
        return Err(format!("the archive carries no {MANIFEST_FILE}"));
    }
    Ok((files, bytes))
}

/// Install one package (a `.zip` written by [`export_plugin`] or zipped by hand).
/// Open a package; anything that is not a zip is refused with its name.
fn open_archive(source: &Path) -> Result<ZipArchive<fs::File>, String> {
    let file = fs::File::open(source).map_err(|err| format!("open {}: {err}", source.display()))?;
    ZipArchive::new(file).map_err(|err| format!("{} is not a zip archive: {err}", source.display()))
}

/// Read the central directory and refuse an archive that is empty or over the caps.
///
/// Sizes come from the directory, so the guard runs before anything is written:
/// a header that claims a terabyte is rejected without unpacking a byte.
fn survey(archive: &mut ZipArchive<fs::File>) -> Result<(Vec<(String, u64, usize)>, u64), String> {
    let mut entries: Vec<(String, u64, usize)> = Vec::new();
    let mut declared = 0u64;
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|err| format!("read entry #{index}: {err}"))?;
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().replace('\\', "/");
        if name.is_empty() {
            continue;
        }
        declared = declared.saturating_add(entry.size());
        entries.push((name, entry.size(), index));
    }
    if entries.is_empty() {
        return Err("the archive holds no files".to_string());
    }
    if entries.len() > MAX_PACKAGE_FILES {
        return Err(format!(
            "the archive holds {} files; a package carries at most {MAX_PACKAGE_FILES}",
            entries.len()
        ));
    }
    if declared > MAX_PACKAGE_BYTES {
        return Err(format!(
            "the archive unpacks to more than {} MB",
            MAX_PACKAGE_BYTES / (1024 * 1024)
        ));
    }
    Ok((entries, declared))
}

/// The manifest an archive carries: the folder it sits in, its entry name, and its parsed form.
fn read_archive_manifest(
    archive: &mut ZipArchive<fs::File>,
    entries: &[(String, u64, usize)],
) -> Result<(String, String, PluginManifest), String> {
    let (prefix, manifest_name) = locate_manifest(entries)?;
    let mut manifest_text = String::new();
    archive
        .by_name(&manifest_name)
        .map_err(|err| format!("read '{manifest_name}': {err}"))?
        .read_to_string(&mut manifest_text)
        .map_err(|err| format!("read '{manifest_name}': {err}"))?;
    let manifest: PluginManifest = serde_json::from_str(&manifest_text)
        .map_err(|err| format!("{manifest_name} is not a valid manifest: {err}"))?;
    Ok((prefix, manifest_name, manifest))
}

/// The id an archive installs as: the manifest's own id, else the wrapper folder, else the file name.
fn derive_id(prefix: &str, manifest: &PluginManifest, source: &Path) -> String {
    let from_manifest = slugify_segment(&manifest.id);
    if !from_manifest.is_empty() {
        return from_manifest;
    }
    let from_folder = slugify_segment(prefix.rsplit('/').next().unwrap_or(""));
    if !from_folder.is_empty() {
        return from_folder;
    }
    slugify_segment(source.file_stem().and_then(|stem| stem.to_str()).unwrap_or(""))
}

/// The id an archive would install as, without writing anything.
///
/// The caller retires that plugin's instance first: Windows refuses to rename a
/// folder a process still has open, so importing over a running plugin would
/// otherwise end in a bare "access denied".
pub fn package_id(source: &Path) -> Option<String> {
    let mut archive = open_archive(source).ok()?;
    let (entries, _) = survey(&mut archive).ok()?;
    let (prefix, _name, manifest) = read_archive_manifest(&mut archive, &entries).ok()?;
    let id = derive_id(&prefix, &manifest, source);
    (!id.is_empty()).then_some(id)
}


pub fn import_plugin(workspace: &Path, source: &Path) -> Result<PluginImportReport, String> {
    let mut archive = open_archive(source)?;
    let (entries, _) = survey(&mut archive)?;
    let (prefix, _manifest_name, manifest) = read_archive_manifest(&mut archive, &entries)?;
    let id = derive_id(&prefix, &manifest, source);
    if id.is_empty() {
        return Err("could not derive a plugin id from the package".to_string());
    }

    let root = plugins_dir(workspace);
    fs::create_dir_all(&root).map_err(|err| format!("create {}: {err}", root.display()))?;
    let registry = load_plugins(workspace);
    let existing = registry.get(&id).is_some();
    if !existing && registry.plugins.len() >= MAX_PLUGINS {
        return Err(format!(
            "the workspace already holds its maximum of {MAX_PLUGINS} plugins; remove one first"
        ));
    }

    // Unpack beside the plugin folder, never into it: a half-written folder
    // would be picked up by the loader while the import is still running.
    let staging = backup_root(workspace).join(format!(".staging-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&staging).map_err(|err| format!("create {}: {err}", staging.display()))?;
    let extracted = extract(&mut archive, &entries, &prefix, &staging).and_then(|counts| {
        check_staged(&staging, &id)?;
        Ok(counts)
    });
    let (files, bytes) = match extracted {
        Ok(counts) => counts,
        Err(err) => {
            let _ = fs::remove_dir_all(&staging);
            return Err(err);
        }
    };

    let target = root.join(&id);
    let backup = if existing {
        let path = backup_path_for(workspace, &id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|err| format!("create {}: {err}", parent.display()))?;
        }
        fs::rename(&target, &path).map_err(|err| format!("move the installed copy aside: {err}"))?;
        Some(path)
    } else {
        None
    };

    if let Err(err) = fs::rename(&staging, &target) {
        if let Some(path) = &backup {
            let _ = fs::rename(path, &target);
        }
        let _ = fs::remove_dir_all(&staging);
        return Err(format!("install the plugin: {err}"));
    }

    // The loader has the last word: a manifest it refuses (id/folder mismatch,
    // bad subscriptions) must not leave a plugin nobody can open.
    let registry = load_plugins(workspace);
    let Some(plugin) = registry.get(&id) else {
        let _ = fs::remove_dir_all(&target);
        if let Some(path) = &backup {
            let _ = fs::rename(path, &target);
        }
        let reason = registry
            .issues
            .iter()
            .find(|issue| issue.dir.ends_with(&id))
            .map(|issue| issue.error.clone())
            .unwrap_or_else(|| "the manifest does not load".to_string());
        return Err(format!("the imported manifest does not load: {reason}"));
    };

    Ok(PluginImportReport {
        plugin: plugin_view(plugin),
        dir: target.to_string_lossy().to_string(),
        files,
        bytes,
        replaced: backup.map(|path| path.to_string_lossy().to_string()),
    })
}

/// Validate the staged folder before it replaces anything on disk.
fn check_staged(staging: &Path, id: &str) -> Result<(), String> {
    let path = staging.join(MANIFEST_FILE);
    let text = fs::read_to_string(&path).map_err(|err| format!("read {MANIFEST_FILE}: {err}"))?;
    let mut manifest: PluginManifest = serde_json::from_str(&text)
        .map_err(|err| format!("{MANIFEST_FILE} is not a valid manifest: {err}"))?;
    manifest.id = id.to_string();
    manifest.validate()?;
    if manifest.command.trim().is_empty() && !manifest.entry.trim().is_empty() {
        let entry = safe_join(staging, &manifest.entry).ok_or_else(|| {
            format!(
                "the entry '{}' is not a path inside the plugin",
                manifest.entry
            )
        })?;
        if !entry.is_file() {
            return Err(format!(
                "the package declares entry '{}' but does not carry it",
                manifest.entry
            ));
        }
    }
    if !manifest.config_schema.trim().is_empty() {
        let schema = safe_join(staging, &manifest.config_schema).ok_or_else(|| {
            format!(
                "the settings schema '{}' is not a path inside the plugin",
                manifest.config_schema
            )
        })?;
        if !schema.is_file() {
            return Err(format!(
                "the package declares the settings schema '{}' but does not carry it",
                manifest.config_schema
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn workspace(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "rabbit-plugin-package-{label}-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(plugins_dir(&dir)).unwrap();
        dir
    }

    fn write(path: &Path, text: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, text).unwrap();
    }

    /// A plugin with a schema, one secret and one preference, plus noise.
    fn demo_plugin(workspace: &Path) -> PathBuf {
        let dir = plugins_dir(workspace).join("demo");
        write(
            &dir.join(MANIFEST_FILE),
            &json!({
                "id": "demo",
                "name": "Demo",
                "version": "1.2.3",
                "runtime": "python",
                "entry": "main.py",
                "tools": [{ "name": "now", "description": "Clock" }],
                "config_schema": "config.schema.yaml"
            })
            .to_string(),
        );
        write(&dir.join("main.py"), "print('hi')\n");
        write(&dir.join("lib/util.py"), "x = 1\n");
        write(
            &dir.join("config.schema.yaml"),
            "sections:\n  - key: main\n    fields:\n      - key: token\n        type: secret\n      - key: region\n        type: text\n",
        );
        write(&dir.join("config.yaml"), "token: super-secret\nregion: eu\n");
        write(&dir.join(".env"), "TOKEN=super-secret\n");
        write(&dir.join("main.py.bak.1"), "old\n");
        write(&dir.join("node_modules/dep/index.js"), "module.exports = 1;\n");
        dir
    }

    fn zip_names(path: &Path) -> Vec<String> {
        let file = fs::File::open(path).unwrap();
        let mut archive = ZipArchive::new(file).unwrap();
        (0..archive.len())
            .map(|index| archive.by_index(index).unwrap().name().to_string())
            .collect()
    }

    fn read_entry(path: &Path, name: &str) -> String {
        let file = fs::File::open(path).unwrap();
        let mut archive = ZipArchive::new(file).unwrap();
        let mut text = String::new();
        archive
            .by_name(name)
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        text
    }

    #[test]
    fn export_packs_payload_and_strips_secrets() {
        let workspace = workspace("export");
        demo_plugin(&workspace);
        let dest = workspace.join("out/demo.zip");

        let report = export_plugin(&workspace, "demo", &dest).unwrap();
        assert_eq!(report.id, "demo");
        assert!(report.settings_exported);
        assert_eq!(report.secrets_removed, 1);
        // `config.yaml` is replaced by the stripped copy, not skipped.
        assert_eq!(report.skipped, 0);
        assert!(report.files >= 4);

        let names = zip_names(&dest);
        for expected in [MANIFEST_FILE, "main.py", "lib/util.py", "config.schema.yaml"] {
            assert!(
                names.iter().any(|name| name == expected),
                "missing {expected}"
            );
        }
        for unexpected in [".env", "main.py.bak.1", "node_modules/dep/index.js"] {
            assert!(
                !names.iter().any(|name| name == unexpected),
                "packed {unexpected}"
            );
        }
        // The id the importer installs as is readable without unpacking anything.
        assert_eq!(package_id(&dest).as_deref(), Some("demo"));

        let settings = read_entry(&dest, "config.yaml");
        assert!(
            !settings.contains("super-secret"),
            "secret leaked: {settings}"
        );
        assert!(settings.contains("region: eu"));
    }

    #[test]
    fn export_without_schema_keeps_settings_out() {
        let workspace = workspace("no-schema");
        let dir = plugins_dir(&workspace).join("plain");
        write(
            &dir.join(MANIFEST_FILE),
            &json!({ "id": "plain", "entry": "main.js" }).to_string(),
        );
        write(&dir.join("main.js"), "console.log(1)\n");
        write(&dir.join("config.yaml"), "token: opaque\n");

        let dest = workspace.join("plain.zip");
        let report = export_plugin(&workspace, "plain", &dest).unwrap();
        assert!(!report.settings_exported);
        assert_eq!(report.skipped, 1);
        assert!(!zip_names(&dest).iter().any(|name| name == "config.yaml"));
    }

    #[test]
    fn export_rejects_an_unknown_plugin() {
        let workspace = workspace("unknown");
        let err = export_plugin(&workspace, "nope", &workspace.join("x.zip")).unwrap_err();
        assert!(err.contains("not installed"), "{err}");
    }

    #[test]
    fn import_restores_the_folder_and_replaces_with_a_backup() {
        let workspace = workspace("roundtrip");
        demo_plugin(&workspace);
        let archive = workspace.join("demo.zip");
        export_plugin(&workspace, "demo", &archive).unwrap();

        fs::remove_dir_all(plugins_dir(&workspace).join("demo")).unwrap();
        let first = import_plugin(&workspace, &archive).unwrap();
        assert_eq!(first.plugin.id, "demo");
        assert!(first.replaced.is_none());
        let dir = plugins_dir(&workspace).join("demo");
        assert!(dir.join("main.py").is_file());
        assert!(dir.join("lib/util.py").is_file());
        let settings = fs::read_to_string(dir.join("config.yaml")).unwrap();
        assert!(settings.contains("region: eu"));

        // A second import replaces the installed copy and keeps the old one.
        let second = import_plugin(&workspace, &archive).unwrap();
        let backup = second.replaced.expect("a replaced copy is kept");
        assert!(Path::new(&backup).join(MANIFEST_FILE).is_file());
        assert!(load_plugins(&workspace).get("demo").is_some());
    }

    #[test]
    fn import_accepts_a_folder_wrapped_archive() {
        let workspace = workspace("wrapped");
        let source = workspace.join("wrapped.zip");
        let file = fs::File::create(&source).unwrap();
        let mut zip = ZipWriter::new(file);
        for (name, text) in [
            (
                "demo/plugin.json",
                json!({ "id": "demo", "entry": "run.sh" }).to_string(),
            ),
            ("demo/run.sh", "echo hi\n".to_string()),
            ("__MACOSX/demo/._run.sh", "junk".to_string()),
            ("demo/.DS_Store", "junk".to_string()),
        ] {
            zip.start_file(name, zip_options()).unwrap();
            zip.write_all(text.as_bytes()).unwrap();
        }
        zip.finish().unwrap();

        let report = import_plugin(&workspace, &source).unwrap();
        assert_eq!(report.plugin.id, "demo");
        let dir = plugins_dir(&workspace).join("demo");
        assert!(dir.join("run.sh").is_file());
        assert!(!dir.join(".DS_Store").exists());
    }

    #[test]
    fn import_refuses_an_archive_without_a_manifest() {
        let workspace = workspace("no-manifest");
        let source = workspace.join("x.zip");
        let file = fs::File::create(&source).unwrap();
        let mut zip = ZipWriter::new(file);
        zip.start_file("readme.txt", zip_options()).unwrap();
        zip.write_all(b"hello").unwrap();
        zip.finish().unwrap();

        let err = import_plugin(&workspace, &source).unwrap_err();
        assert!(err.contains(MANIFEST_FILE), "{err}");
    }

    #[test]
    fn import_refuses_two_plugins_in_one_archive() {
        let workspace = workspace("two");
        let source = workspace.join("two.zip");
        let file = fs::File::create(&source).unwrap();
        let mut zip = ZipWriter::new(file);
        for name in ["a/plugin.json", "b/plugin.json"] {
            zip.start_file(name, zip_options()).unwrap();
            zip.write_all(b"{}").unwrap();
        }
        zip.finish().unwrap();

        let err = import_plugin(&workspace, &source).unwrap_err();
        assert!(err.contains("one package at a time"), "{err}");
    }

    #[test]
    fn import_refuses_a_missing_entry_and_leaves_nothing_behind() {
        let workspace = workspace("missing-entry");
        let source = workspace.join("broken.zip");
        let file = fs::File::create(&source).unwrap();
        let mut zip = ZipWriter::new(file);
        zip.start_file(MANIFEST_FILE, zip_options()).unwrap();
        zip.write_all(json!({ "id": "broken", "entry": "main.py" }).to_string().as_bytes())
            .unwrap();
        zip.finish().unwrap();

        let err = import_plugin(&workspace, &source).unwrap_err();
        assert!(err.contains("main.py"), "{err}");
        assert!(!plugins_dir(&workspace).join("broken").exists());
        let staging = fs::read_dir(backup_root(&workspace))
            .map(|dir| dir.count())
            .unwrap_or(0);
        assert_eq!(staging, 0, "staging folders are cleaned up");
    }

    #[test]
    fn safe_join_rejects_traversal() {
        let root = Path::new("/tmp/plugin");
        assert!(safe_join(root, "lib/util.py").is_some());
        assert!(safe_join(root, "../escape.py").is_none());
        assert!(safe_join(root, "lib/../../escape.py").is_none());
        assert!(safe_join(root, "/etc/passwd").is_none());
    }
}
