use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Mutex,
};

use crate::{
    model::validate_id, ExposeSecret, Profile, Profiles, RemoteError, Result, SecretToken,
};

const MAX_CONFIG_BYTES: u64 = 1024 * 1024;
// Credential stores (notably Windows) do not promise write ordering. This also
// serializes separate views/instances accessing the same profile/account.
static VAULT_LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone, Debug)]
pub struct ProfileStore {
    path: PathBuf,
}

impl ProfileStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn open_default() -> Result<Self> {
        if let Some(path) = std::env::var_os("MANGROVE_PROFILES_PATH") {
            return Ok(Self::new(path));
        }
        let root = dirs::config_dir().ok_or(RemoteError::Storage)?;
        Ok(Self::new(root.join("mangrove-gpui").join("profiles.json")))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> Result<Profiles> {
        let file = match File::open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Profiles::default())
            }
            Err(_) => return Err(RemoteError::Storage),
        };
        let mut bytes = Vec::new();
        file.take(MAX_CONFIG_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| RemoteError::Storage)?;
        if bytes.len() as u64 > MAX_CONFIG_BYTES {
            return Err(RemoteError::Storage);
        }
        let profiles: Profiles =
            serde_json::from_slice(&bytes).map_err(|_| RemoteError::Storage)?;
        profiles.validate()?;
        Ok(profiles)
    }

    pub fn save(&self, profiles: &Profiles) -> Result<()> {
        profiles.validate()?;
        let bytes = serde_json::to_vec_pretty(profiles).map_err(|_| RemoteError::Storage)?;
        if bytes.len() as u64 > MAX_CONFIG_BYTES {
            return Err(RemoteError::Storage);
        }
        let parent = self
            .path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .ok_or(RemoteError::Storage)?;
        fs::create_dir_all(parent).map_err(|_| RemoteError::Storage)?;
        let mut temporary =
            tempfile::NamedTempFile::new_in(parent).map_err(|_| RemoteError::Storage)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            temporary
                .as_file()
                .set_permissions(fs::Permissions::from_mode(0o600))
                .map_err(|_| RemoteError::Storage)?;
        }
        temporary
            .write_all(&bytes)
            .map_err(|_| RemoteError::Storage)?;
        temporary
            .as_file()
            .sync_all()
            .map_err(|_| RemoteError::Storage)?;
        temporary
            .persist(&self.path)
            .map_err(|_| RemoteError::Storage)?;
        Ok(())
    }
}

pub trait CredentialVault: Send + Sync {
    fn store(&self, profile_id: &str, account_id: &str, token: &SecretToken) -> Result<()>;
    fn read(&self, profile_id: &str, account_id: &str) -> Result<Option<SecretToken>>;
    fn delete(&self, profile_id: &str, account_id: &str) -> Result<()>;
    /// Replace a rotated OAuth credential only if its original entry still
    /// exists and is unchanged, serialized with this process's vault operations.
    /// A concurrent logout in this application must never be undone. Native OS
    /// keyrings do not provide CAS against other application processes.
    /// Custom vaults without atomic replacement fail closed for OAuth refresh.
    fn replace_if_matches(
        &self,
        _profile_id: &str,
        _account_id: &str,
        _previous: &SecretToken,
        _replacement: &SecretToken,
    ) -> Result<bool> {
        Err(RemoteError::VaultUnavailable)
    }
}

/// Deletes the credential of every account in `profile`. Vaults treat a
/// missing entry as deleted, so a partially failed cleanup can be retried.
pub fn delete_profile_credentials(vault: &dyn CredentialVault, profile: &Profile) -> Result<()> {
    profile
        .accounts
        .iter()
        .try_for_each(|account| vault.delete(&profile.id, &account.id))
}

/// Explicit native backends are enabled per platform in Cargo.toml. A missing
/// or locked OS service is an error, with no file or in-memory production fallback.
#[derive(Default)]
pub struct OsCredentialVault;

impl OsCredentialVault {
    pub fn new() -> Self {
        Self
    }
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
fn vault_entry(profile_id: &str, account_id: &str) -> Result<keyring::Entry> {
    validate_id(profile_id)?;
    validate_id(account_id)?;
    keyring::Entry::new("dev.mangrove.remote", &format!("{profile_id}:{account_id}"))
        .map_err(|_| RemoteError::VaultUnavailable)
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
impl CredentialVault for OsCredentialVault {
    fn store(&self, profile_id: &str, account_id: &str, token: &SecretToken) -> Result<()> {
        let _guard = VAULT_LOCK
            .lock()
            .map_err(|_| RemoteError::VaultUnavailable)?;
        if token.expose_secret().is_empty() || token.expose_secret().len() > 16 * 1024 {
            return Err(RemoteError::InvalidConfiguration);
        }
        vault_entry(profile_id, account_id)?
            .set_password(token.expose_secret())
            .map_err(|_| RemoteError::VaultUnavailable)
    }

    fn read(&self, profile_id: &str, account_id: &str) -> Result<Option<SecretToken>> {
        let _guard = VAULT_LOCK
            .lock()
            .map_err(|_| RemoteError::VaultUnavailable)?;
        match vault_entry(profile_id, account_id)?.get_password() {
            Ok(value) => Ok(Some(SecretToken::from(value))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err(RemoteError::VaultUnavailable),
        }
    }

    fn delete(&self, profile_id: &str, account_id: &str) -> Result<()> {
        let _guard = VAULT_LOCK
            .lock()
            .map_err(|_| RemoteError::VaultUnavailable)?;
        match vault_entry(profile_id, account_id)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err(RemoteError::VaultUnavailable),
        }
    }

    fn replace_if_matches(
        &self,
        profile_id: &str,
        account_id: &str,
        previous: &SecretToken,
        replacement: &SecretToken,
    ) -> Result<bool> {
        let _guard = VAULT_LOCK
            .lock()
            .map_err(|_| RemoteError::VaultUnavailable)?;
        if replacement.expose_secret().is_empty() || replacement.expose_secret().len() > 16 * 1024 {
            return Err(RemoteError::InvalidConfiguration);
        }
        let entry = vault_entry(profile_id, account_id)?;
        let current = match entry.get_password() {
            Ok(value) => SecretToken::from(value),
            Err(keyring::Error::NoEntry) => return Ok(false),
            Err(_) => return Err(RemoteError::VaultUnavailable),
        };
        if current.expose_secret() != previous.expose_secret() {
            return Ok(false);
        }
        entry
            .set_password(replacement.expose_secret())
            .map_err(|_| RemoteError::VaultUnavailable)?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Account, Provider};

    fn account(server: &str) -> Account {
        Account {
            id: "same-account".into(),
            label: "Compte".into(),
            provider: Provider::GitLab,
            server_url: server.into(),
            username: None,
            authentication: crate::Authentication::PersonalToken,
        }
    }

    #[test]
    fn profile_store_roundtrips_distinct_profiles_without_serializing_credentials() {
        let directory = tempfile::tempdir().unwrap();
        let store = ProfileStore::new(directory.path().join("mangrove/profiles.json"));
        let mut profiles = store.load().unwrap();
        profiles.profiles[0]
            .accounts
            .push(account("https://personal.example"));
        profiles.profiles[1]
            .accounts
            .push(account("https://professional.example"));
        profiles.profiles[0]
            .favorites
            .push("personal/project".into());
        profiles.active_profile_id = "professional".into();
        store.save(&profiles).unwrap();
        assert_eq!(store.load().unwrap(), profiles);
        let text = fs::read_to_string(store.path()).unwrap();
        for forbidden in ["password", "token", "secret"] {
            assert!(!text.contains(forbidden));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(store.path()).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        // Atomic replacement continues working on subsequent changes.
        profiles.active_profile_id = "personal".into();
        store.save(&profiles).unwrap();
        assert_eq!(store.load().unwrap(), profiles);
    }

    #[test]
    fn invalid_profiles_do_not_replace_the_previous_configuration() {
        let directory = tempfile::tempdir().unwrap();
        let store = ProfileStore::new(directory.path().join("profiles.json"));
        let original = Profiles::default();
        store.save(&original).unwrap();
        let mut invalid = original.clone();
        invalid.profiles[0]
            .accounts
            .push(account("https://token@example.test"));
        assert_eq!(store.save(&invalid), Err(RemoteError::InvalidConfiguration));
        assert_eq!(store.load().unwrap(), original);
        invalid = original.clone();
        invalid.profiles[1].id = invalid.profiles[0].id.clone();
        assert_eq!(store.save(&invalid), Err(RemoteError::InvalidConfiguration));
        invalid = original.clone();
        invalid.profiles[0].id = "../another-profile".into();
        assert_eq!(store.save(&invalid), Err(RemoteError::InvalidConfiguration));
    }

    #[test]
    fn malformed_unknown_secret_fields_and_oversized_configuration_are_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let store = ProfileStore::new(directory.path().join("profiles.json"));
        fs::write(store.path(), b"{broken").unwrap();
        assert_eq!(store.load(), Err(RemoteError::Storage));
        let mut value = serde_json::to_value(Profiles::default()).unwrap();
        value["profiles"][0]["token"] = "must-not-load".into();
        fs::write(store.path(), serde_json::to_vec(&value).unwrap()).unwrap();
        assert_eq!(store.load(), Err(RemoteError::Storage));
        fs::write(store.path(), vec![b' '; MAX_CONFIG_BYTES as usize + 1]).unwrap();
        assert_eq!(store.load(), Err(RemoteError::Storage));
    }

    #[test]
    fn deleting_profile_credentials_removes_only_that_profile_entries() {
        use std::collections::HashMap;
        #[derive(Default)]
        struct MemoryVault(Mutex<HashMap<(String, String), String>>);
        impl CredentialVault for MemoryVault {
            fn store(&self, profile_id: &str, account_id: &str, token: &SecretToken) -> Result<()> {
                self.0.lock().unwrap().insert(
                    (profile_id.into(), account_id.into()),
                    token.expose_secret().into(),
                );
                Ok(())
            }
            fn read(&self, profile_id: &str, account_id: &str) -> Result<Option<SecretToken>> {
                Ok(self
                    .0
                    .lock()
                    .unwrap()
                    .get(&(profile_id.into(), account_id.into()))
                    .map(|token| SecretToken::from(token.clone())))
            }
            fn delete(&self, profile_id: &str, account_id: &str) -> Result<()> {
                self.0
                    .lock()
                    .unwrap()
                    .remove(&(profile_id.into(), account_id.into()));
                Ok(())
            }
        }
        let vault = MemoryVault::default();
        let mut work = Profile::new("professional", "Professionnel");
        work.accounts.push(account("https://gitlab.example"));
        let token = SecretToken::from("synthetic-fixture");
        vault.store("professional", "same-account", &token).unwrap();
        vault.store("personal", "same-account", &token).unwrap();
        delete_profile_credentials(&vault, &work).unwrap();
        assert!(vault
            .read("professional", "same-account")
            .unwrap()
            .is_none());
        assert!(vault.read("personal", "same-account").unwrap().is_some());
        // Already deleted entries do not prevent a retry.
        delete_profile_credentials(&vault, &work).unwrap();
    }

    #[test]
    fn vault_invalid_identifiers_are_rejected_without_contacting_the_os() {
        let vault = OsCredentialVault::new();
        assert_eq!(
            vault.delete("../profile", "account"),
            Err(RemoteError::InvalidConfiguration)
        );
        assert!(matches!(
            vault.read("profile", "account:other"),
            Err(RemoteError::InvalidConfiguration)
        ));
    }

    #[test]
    #[ignore = "opt-in QA: requires an unlocked native OS credential vault"]
    fn os_vault_roundtrip_with_unique_synthetic_identity() {
        use std::time::{SystemTime, UNIX_EPOCH};
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let profile_id = format!("qa-{}-{suffix}", std::process::id());
        let account_id = "synthetic-credential";
        struct Cleanup(String);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = OsCredentialVault::new().delete(&self.0, "synthetic-credential");
            }
        }
        let _cleanup = Cleanup(profile_id.clone());
        let vault = OsCredentialVault::new();
        let fixture = SecretToken::from("mangrove-synthetic-qa-token");
        vault.store(&profile_id, account_id, &fixture).unwrap();
        let loaded = vault.read(&profile_id, account_id).unwrap().unwrap();
        assert!(loaded.expose_secret() == fixture.expose_secret());
        assert!(!format!("{loaded:?}").contains(fixture.expose_secret()));
        let rotated = SecretToken::from("mangrove-synthetic-qa-rotated");
        assert!(!vault
            .replace_if_matches(&profile_id, account_id, &rotated, &fixture)
            .unwrap());
        assert!(
            vault
                .read(&profile_id, account_id)
                .unwrap()
                .unwrap()
                .expose_secret()
                == fixture.expose_secret()
        );
        assert!(vault
            .replace_if_matches(&profile_id, account_id, &fixture, &rotated)
            .unwrap());
        assert!(
            vault
                .read(&profile_id, account_id)
                .unwrap()
                .unwrap()
                .expose_secret()
                == rotated.expose_secret()
        );
        vault.delete(&profile_id, account_id).unwrap();
        assert!(!vault
            .replace_if_matches(&profile_id, account_id, &rotated, &fixture)
            .unwrap());
        assert!(vault.read(&profile_id, account_id).unwrap().is_none());
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
impl CredentialVault for OsCredentialVault {
    fn store(&self, _: &str, _: &str, _: &SecretToken) -> Result<()> {
        Err(RemoteError::VaultUnavailable)
    }
    fn read(&self, _: &str, _: &str) -> Result<Option<SecretToken>> {
        Err(RemoteError::VaultUnavailable)
    }
    fn delete(&self, _: &str, _: &str) -> Result<()> {
        Err(RemoteError::VaultUnavailable)
    }
}
