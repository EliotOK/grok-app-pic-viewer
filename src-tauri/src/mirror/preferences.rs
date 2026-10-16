//! Desktop-owned pairing and policy, retained across application restarts.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::{normalize_max_clients, DEFAULT_MAX_CLIENTS};

#[derive(Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(super) struct Preferences {
    pub enabled: bool,
    pub read_only: bool,
    pub allow_lan: bool,
    pub max_clients: u32,
    pub token: Option<String>,
    pub port: Option<u16>,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            enabled: false,
            read_only: true,
            allow_lan: false,
            max_clients: DEFAULT_MAX_CLIENTS,
            token: None,
            port: None,
        }
    }
}

pub(super) fn load(path: &Path) -> Result<Preferences, String> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Preferences::default()),
        Err(e) => return Err(format!("read mirror preferences: {e}")),
    };
    // Do not include serde's error text: a malformed value may contain the token.
    let mut saved: Preferences = serde_json::from_slice(&bytes)
        .map_err(|_| "invalid mirror preferences; repair mirror-host.json".to_string())?;
    saved.max_clients = normalize_max_clients(saved.max_clients);
    saved.port = saved.port.filter(|port| *port > 0);
    saved.token = saved.token.filter(|token| {
        token.len() >= 32
            && token.len() <= 256
            && token
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
    });
    Ok(saved)
}

pub(super) fn save(path: &Path, saved: &Preferences) -> Result<(), String> {
    crate::store_lock::with_exclusive_lock(path, || {
        let tmp = path.with_extension("json.tmp");
        let bytes = serde_json::to_vec_pretty(saved).map_err(|_| "encode mirror preferences")?;
        let mut options = OpenOptions::new();
        options.create(true).truncate(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let result = (|| {
            let mut file = options.open(&tmp)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                file.set_permissions(fs::Permissions::from_mode(0o600))?;
            }
            file.write_all(&bytes)?;
            file.sync_all()?;
            drop(file);
            fs::rename(&tmp, path)
        })();
        result.map_err(|e| {
            let _ = fs::remove_file(&tmp);
            format!("save mirror preferences: {e}")
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_local_read_only_and_stopped() {
        let saved: Preferences = serde_json::from_str("{}").unwrap();
        assert!(!saved.enabled);
        assert!(saved.read_only);
        assert!(!saved.allow_lan);
        assert_eq!(saved.max_clients, 4);
    }

    #[test]
    fn pairing_and_policy_round_trip_and_replace() {
        let dir = std::env::temp_dir().join(format!("mirror-prefs-{}", uuid::Uuid::new_v4()));
        let path = dir.join("mirror-host.json");
        let mut saved = Preferences {
            enabled: true,
            read_only: false,
            allow_lan: true,
            max_clients: 7,
            token: Some("test-pairing-token-0123456789abcdef".into()),
            port: Some(63867),
        };
        save(&path, &saved).unwrap();
        let loaded = load(&path).unwrap();
        assert!(loaded.enabled && loaded.allow_lan && !loaded.read_only);
        assert_eq!(loaded.max_clients, 7);
        assert_eq!(loaded.token, saved.token);
        assert_eq!(loaded.port, saved.port);
        saved.enabled = false;
        save(&path, &saved).unwrap();
        assert!(!load(&path).unwrap().enabled);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        fs::remove_dir_all(dir).unwrap();
    }
}
