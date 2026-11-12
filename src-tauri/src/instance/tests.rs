//! Unit tests for `instance.rs`.

use super::*;

fn temp_root(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "rabbit-chat-studio-instance-{label}-{}",
        uuid::Uuid::new_v4()
    ));
    fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn write_config(path: &Path, config: &AppConfig) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).ok();
    }
    fs::write(path, serde_json::to_string_pretty(config).unwrap()).expect("config write");
}

/// A live sibling keeps the primary role away: this process ends up as a
/// shadow without touching the main config.
#[test]
fn a_live_primary_makes_the_next_process_a_shadow() {
    let root = temp_root("claim-live");
    fs::create_dir_all(root.join(SHADOW_DIR)).unwrap();
    fs::write(
        root.join(SHADOW_DIR).join(PRIMARY_LOCK_FILE),
        r#"{"pid":4242,"started_at_ms":0}"#,
    )
    .unwrap();

    let instance = Instance::claim_with(root.clone(), &|pid| pid == 4242);
    assert_eq!(instance.role(), Role::Shadow);
    assert_eq!(instance.config_path(), root.join("config.json"));
    fs::remove_dir_all(root).ok();
}

/// A crashed run leaves its lock behind; the next process must take over
/// instead of nesting every later window one level deeper.
#[test]
fn a_dead_owner_is_reclaimed() {
    let root = temp_root("claim-dead");
    fs::create_dir_all(root.join(SHADOW_DIR)).unwrap();
    let lock = root.join(SHADOW_DIR).join(PRIMARY_LOCK_FILE);
    fs::write(&lock, r#"{"pid":4243,"started_at_ms":0}"#).unwrap();

    let instance = Instance::claim_with(root.clone(), &|_| false);
    assert_eq!(instance.role(), Role::Primary);
    let owner = read_owner(&lock).expect("the lock names its new owner");
    assert_eq!(owner.pid, std::process::id());
    fs::remove_dir_all(root).ok();
}

/// The shadow copy is a fork: its `workspace_dir` points into its own
/// directory, the shared config is left alone, and `prepare` is idempotent.
#[test]
fn a_shadow_forks_its_workspace_into_its_own_directory() {
    let root = temp_root("prepare");
    fs::create_dir_all(root.join(SHADOW_DIR)).unwrap();
    fs::write(
        root.join(SHADOW_DIR).join(PRIMARY_LOCK_FILE),
        r#"{"pid":4244,"started_at_ms":0}"#,
    )
    .unwrap();
    let mut main = AppConfig::default();
    main.workspace_dir = Some("D:/shared".to_string());
    main.workspace_history = vec!["D:/shared".to_string()];
    write_config(&root.join("config.json"), &main);

    let mut instance = Instance::claim_with(root.clone(), &|pid| pid == 4244);
    let config_path = instance.prepare().expect("shadow config");
    assert_ne!(config_path, root.join("config.json"));

    let copied: AppConfig =
        serde_json::from_str(&fs::read_to_string(&config_path).unwrap()).unwrap();
    let workspace = config_path.parent().unwrap().join("workspace");
    assert_eq!(
        copied.workspace_dir.as_deref(),
        Some(workspace.to_string_lossy().as_ref())
    );
    assert!(workspace.is_dir(), "the forked workspace exists");
    // The history travels with the copy (the dropdown stays useful)…
    assert_eq!(copied.workspace_history, vec!["D:/shared".to_string()]);
    // …while the shared config keeps its own workspace.
    let shared: AppConfig =
        serde_json::from_str(&fs::read_to_string(root.join("config.json")).unwrap()).unwrap();
    assert_eq!(shared.workspace_dir.as_deref(), Some("D:/shared"));

    assert_eq!(instance.prepare().unwrap(), config_path, "idempotent");
    assert!(instance.shadow_id().is_some());
    fs::remove_dir_all(root).ok();
}

/// A shadow's exit merges only the workspace history, never the settings it
/// was holding, and removes its directory.
#[test]
fn a_shadow_merges_only_the_workspace_history() {
    let root = temp_root("merge");
    fs::create_dir_all(root.join(SHADOW_DIR)).unwrap();
    fs::write(
        root.join(SHADOW_DIR).join(PRIMARY_LOCK_FILE),
        r#"{"pid":4245,"started_at_ms":0}"#,
    )
    .unwrap();
    let mut main = AppConfig::default();
    main.api_key = "keep-me".to_string();
    main.api_base_url = "https://shared.example/v1".to_string();
    main.workspace_history = vec!["C:/shared".to_string()];
    write_config(&root.join("config.json"), &main);

    let mut instance = Instance::claim_with(root.clone(), &|pid| pid == 4245);
    let config_path = instance.prepare().unwrap();
    // The shadow's own config: a different key and a newer workspace.
    let mut shadow: AppConfig =
        serde_json::from_str(&fs::read_to_string(&config_path).unwrap()).unwrap();
    shadow.api_key = "shadow-only".to_string();
    shadow.workspace_history = vec!["D:/shadow".to_string(), "C:/shared".to_string()];
    fs::write(&config_path, serde_json::to_string_pretty(&shadow).unwrap()).unwrap();
    let shadow_dir = config_path.parent().unwrap().to_path_buf();

    instance.shutdown();

    let merged: AppConfig =
        serde_json::from_str(&fs::read_to_string(root.join("config.json")).unwrap()).unwrap();
    assert_eq!(merged.api_key, "keep-me", "settings stay with the primary");
    assert_eq!(merged.api_base_url, "https://shared.example/v1");
    assert_eq!(
        merged.workspace_history,
        vec!["D:/shadow".to_string(), "C:/shared".to_string()],
        "the shadow's newest workspace heads the shared history"
    );
    assert!(!shadow_dir.exists(), "the shadow copy is removed on exit");
    fs::remove_dir_all(root).ok();
}

/// The fork directory is temporary: it must not show up in the shared
/// history, while a workspace the user picked in that window still does.
#[test]
fn a_shadow_does_not_share_its_own_temporary_workspace() {
    let root = temp_root("merge-fork");
    fs::create_dir_all(root.join(SHADOW_DIR)).unwrap();
    fs::write(
        root.join(SHADOW_DIR).join(PRIMARY_LOCK_FILE),
        r#"{"pid":4246,"started_at_ms":0}"#,
    )
    .unwrap();
    write_config(&root.join("config.json"), &AppConfig::default());

    let mut instance = Instance::claim_with(root.clone(), &|pid| pid == 4246);
    let config_path = instance.prepare().unwrap();
    let mut shadow: AppConfig =
        serde_json::from_str(&fs::read_to_string(&config_path).unwrap()).unwrap();
    let fork = shadow.workspace_dir.clone().expect("the fork is recorded");
    shadow.workspace_history = vec!["D:/real".to_string(), fork];
    fs::write(&config_path, serde_json::to_string_pretty(&shadow).unwrap()).unwrap();

    instance.shutdown();

    let merged: AppConfig =
        serde_json::from_str(&fs::read_to_string(root.join("config.json")).unwrap()).unwrap();
    assert_eq!(
        merged.workspace_history,
        vec!["D:/real".to_string()],
        "only a workspace the user really picked is shared"
    );
    fs::remove_dir_all(root).ok();
}

/// Running the exit hook twice (window close, then app exit) must not merge
/// or delete twice.
#[test]
fn shutdown_is_idempotent() {
    let root = temp_root("shutdown-twice");
    let instance = Instance::claim_with(root.clone(), &|_| false);
    instance.shutdown();
    let lock = root.join(SHADOW_DIR).join(PRIMARY_LOCK_FILE);
    assert!(!lock.exists(), "the primary drops its lock");
    // A second call has nothing left to do and must not fail.
    instance.shutdown();
    // A later launch can still become the primary.
    let again = Instance::claim_with(root.clone(), &|_| false);
    assert_eq!(again.role(), Role::Primary);
    fs::remove_dir_all(root).ok();
}

/// Startup GC drops leftovers of killed windows and keeps live siblings.
#[test]
fn garbage_collection_spares_live_shadows() {
    let root = temp_root("gc");
    let live = root.join(SHADOW_DIR).join("config-20260101T101530-aaaaaaaa");
    let dead = root.join(SHADOW_DIR).join("config-20260101T101531-bbbbbbbb");
    for dir in [&live, &dead] {
        fs::create_dir_all(dir.join("workspace")).unwrap();
        fs::write(dir.join("config.json"), "{}").unwrap();
    }
    write_owner(&live, std::process::id()).unwrap();
    write_owner(&dead, u32::MAX - 1).unwrap();

    assert_eq!(collect_garbage(&root), 1);
    assert!(live.is_dir(), "a running window's copy is never touched");
    assert!(!dead.exists());
    fs::remove_dir_all(root).ok();
}

#[test]
fn shadow_names_are_sorted_by_time_and_unique() {
    let name = new_shadow_name();
    assert!(name.starts_with(SHADOW_PREFIX));
    let id = name.strip_prefix(SHADOW_PREFIX).unwrap();
    let (stamp, random) = id.split_once('-').expect("stamp-random");
    assert_eq!(stamp.len(), "20260101T101530".len(), "sortable timestamp");
    assert_eq!(random.len(), 8, "short random suffix");
    assert_ne!(name, new_shadow_name());
}

#[test]
fn a_shadow_window_title_is_tagged() {
    let root = temp_root("title");
    let mut instance = Instance::claim_with(root.join("app"), &|_| false);
    // Primary: the historical title, unchanged.
    assert_eq!(
        instance.window_title(Path::new("/ws")),
        "Rabbit Chat Studio — /ws"
    );
    instance.role = Role::Shadow;
    instance.shadow_dir = Some(root.join("app").join(SHADOW_DIR).join("config-1-a1b2c3d4"));
    let title = instance.window_title(Path::new("/ws"));
    assert!(title.contains("[shadow a1b2c3d4]"), "{title}");
    fs::remove_dir_all(root).ok();
}

/// Clearing `workspace_dir` must not send a shadow to the primary's
/// default workspace: that would put two windows on one database.
#[test]
fn a_shadow_defaults_to_its_own_fork() {
    // Primary: the shared app-data workspace.
    let primary_root = temp_root("default-workspace-primary");
    let primary = Instance::claim_with(primary_root.clone(), &|_| false);
    let shared = primary_root.join("workspace");
    assert_eq!(primary.default_workspace(shared.clone()), shared);
    primary.shutdown();
    fs::remove_dir_all(primary_root).ok();

    // Shadow: its own fork once prepared.
    let root = temp_root("default-workspace-shadow");
    fs::create_dir_all(root.join(SHADOW_DIR)).unwrap();
    fs::write(
        root.join(SHADOW_DIR).join(PRIMARY_LOCK_FILE),
        r#"{"pid":4247,"started_at_ms":0}"#,
    )
    .unwrap();
    write_config(&root.join("config.json"), &AppConfig::default());

    let mut instance = Instance::claim_with(root.clone(), &|pid| pid == 4247);
    let config_path = instance.prepare().unwrap();
    let fork = config_path.parent().unwrap().join("workspace");
    assert_eq!(instance.default_workspace(shared), fork);
    fs::remove_dir_all(root).ok();
}

