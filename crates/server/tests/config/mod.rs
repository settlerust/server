use super::{Auth, Config, ConfigInput, ErrorCode, local_key};
use std::path::PathBuf;

#[test]
fn defaults_start_a_local_server_with_loopback_binding() {
    let config = Config::parse(ConfigInput::default()).unwrap();
    assert_eq!(config.bind, "127.0.0.1:8080".parse().unwrap());
    assert_eq!(config.data_dir, PathBuf::from("data"));
    assert!(config.local);
    assert!(!config.aws);
}

#[test]
fn explicit_values_select_mode_storage_address_and_data_directory() {
    let config = Config::parse(ConfigInput {
        mode: Some("production".into()),
        bind: Some("[::1]:9000".into()),
        storage: Some("aws".into()),
        data_dir: Some(PathBuf::from("custom-data")),
    })
    .unwrap();
    assert_eq!(config.bind, "[::1]:9000".parse().unwrap());
    assert_eq!(config.data_dir, PathBuf::from("custom-data"));
    assert!(!config.local);
    assert!(config.aws);

    let config = Config::parse(ConfigInput {
        mode: Some("local".into()),
        storage: Some("local".into()),
        ..ConfigInput::default()
    })
    .unwrap();
    assert!(config.local);
    assert!(!config.aws);
}

#[test]
fn invalid_modes_storage_choices_and_socket_addresses_are_rejected() {
    for (name, values) in [
        ("SETTLERS_MODE", ["", "Production", "local ", "unknown"]),
        ("SETTLERS_STORAGE", ["", "AWS", "sqlite", "unknown"]),
        (
            "SETTLERS_BIND",
            ["", "localhost:8080", "127.0.0.1:65536", "127.0.0.1"],
        ),
    ] {
        for value in values {
            let mut input = ConfigInput::default();
            match name {
                "SETTLERS_MODE" => input.mode = Some(value.into()),
                "SETTLERS_STORAGE" => input.storage = Some(value.into()),
                _ => input.bind = Some(value.into()),
            }
            let error = Config::parse(input).err().expect("invalid input accepted");
            assert_eq!(error.0.code, ErrorCode::InvalidRequest);
            assert!(error.0.message.contains(name));
        }
    }
}

#[test]
fn a_generated_signing_key_is_reused_across_restarts() {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path().join("nested/data");
    let key = local_key(&data_dir).unwrap();
    assert_eq!(key.len(), 32);
    assert_eq!(local_key(&data_dir).unwrap(), key);
    assert_eq!(std::fs::read(data_dir.join("auth.key")).unwrap(), key);
}

#[cfg(unix)]
#[test]
fn a_new_signing_key_is_readable_only_by_its_owner() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    local_key(dir.path()).unwrap();
    let permissions = std::fs::metadata(dir.path().join("auth.key"))
        .unwrap()
        .permissions();
    assert_eq!(permissions.mode() & 0o777, 0o600);
}

#[test]
fn an_invalid_existing_key_is_preserved_and_rejected_for_authentication() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("auth.key");
    std::fs::write(&path, b"truncated").unwrap();
    let key = local_key(dir.path()).unwrap();
    assert_eq!(key, b"truncated");
    let error = Auth::new(key, true).err().unwrap();
    assert_eq!(error.0.code, ErrorCode::InvalidRequest);
    assert_eq!(std::fs::read(path).unwrap(), b"truncated");
}

#[test]
fn filesystem_errors_do_not_replace_existing_paths() {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path().join("not-a-directory");
    std::fs::write(&data_dir, b"existing file").unwrap();
    assert_eq!(
        local_key(&data_dir).unwrap_err().0.code,
        ErrorCode::StorageUnavailable
    );
    assert_eq!(std::fs::read(data_dir).unwrap(), b"existing file");

    let key_path = dir.path().join("auth.key");
    std::fs::create_dir(&key_path).unwrap();
    assert_eq!(
        local_key(dir.path()).unwrap_err().0.code,
        ErrorCode::StorageUnavailable
    );
    assert!(key_path.is_dir());
}
