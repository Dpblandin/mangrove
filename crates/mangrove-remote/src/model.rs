use std::{collections::HashSet, error::Error, fmt};

use serde::{Deserialize, Serialize};

use crate::{SafeApiUrl, SshChoice};

pub type Result<T> = std::result::Result<T, RemoteError>;

/// Errors deliberately contain neither upstream bodies nor credential URLs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemoteError {
    Cancelled,
    InvalidConfiguration,
    Network,
    Timeout,
    CredentialsExpired,
    RateLimited { retry_after_seconds: Option<u64> },
    Http { status: u16 },
    InvalidResponse,
    ResponseTooLarge,
    UnsafePagination,
    VaultUnavailable,
    Storage,
    SshUnavailable,
    SshAuthentication,
    SshHostKey,
}

impl fmt::Display for RemoteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::Cancelled => "Lecture annulée.",
            Self::InvalidConfiguration => "Configuration distante invalide.",
            Self::Network => "Connexion au serveur impossible.",
            Self::Timeout => "Le serveur n’a pas répondu dans le délai imparti.",
            Self::CredentialsExpired => "Identifiants expirés ou révoqués.",
            Self::RateLimited { .. } => "Limite de l’API atteinte. Réessayez plus tard.",
            Self::Http { .. } => "Le serveur a refusé la lecture.",
            Self::InvalidResponse => "Réponse du serveur non prise en charge.",
            Self::ResponseTooLarge => "La réponse dépasse la limite de lecture.",
            Self::UnsafePagination => "Le serveur a fourni une pagination non sûre.",
            Self::VaultUnavailable => "Le coffre sécurisé de l’OS est indisponible.",
            Self::Storage => "Impossible de lire ou enregistrer les profils.",
            Self::SshUnavailable => "Git ou le transport SSH est indisponible.",
            Self::SshAuthentication => "La connexion SSH a été refusée.",
            Self::SshHostKey => "La clé du serveur SSH est absente ou différente de la clé connue.",
        };
        f.write_str(text)
    }
}

impl Error for RemoteError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Provider {
    GitLab,
    GitHub,
    BitbucketCloud,
    BitbucketServer,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Account {
    pub id: String,
    pub label: String,
    pub provider: Provider,
    pub server_url: String,
    pub username: Option<String>,
}

impl fmt::Debug for Account {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Account")
            .field("id", &self.id)
            .field("label", &self.label)
            .field("provider", &self.provider)
            .field("server_url", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

impl Account {
    pub fn validate(&self) -> Result<()> {
        validate_id(&self.id)?;
        validate_text(&self.label, 128)?;
        if self.server_url.len() > 2048 {
            return Err(RemoteError::InvalidConfiguration);
        }
        SafeApiUrl::new(&self.server_url, "")?;
        if let Some(username) = &self.username {
            validate_text(username, 256)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub accounts: Vec<Account>,
    #[serde(default)]
    pub favorites: Vec<String>,
    #[serde(default)]
    pub ssh: SshChoice,
}

impl Profile {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            accounts: Vec::new(),
            favorites: Vec::new(),
            ssh: SshChoice::default(),
        }
    }

    pub fn validate(&self) -> Result<()> {
        validate_id(&self.id)?;
        validate_text(&self.name, 128)?;
        if self.accounts.len() > 32 || self.favorites.len() > 256 {
            return Err(RemoteError::InvalidConfiguration);
        }
        let mut ids = HashSet::new();
        for account in &self.accounts {
            account.validate()?;
            if !ids.insert(&account.id) {
                return Err(RemoteError::InvalidConfiguration);
            }
        }
        for favorite in &self.favorites {
            validate_text(favorite, 2048)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profiles {
    pub active_profile_id: String,
    pub profiles: Vec<Profile>,
}

impl Default for Profiles {
    fn default() -> Self {
        Self {
            active_profile_id: "personal".into(),
            profiles: vec![
                Profile::new("personal", "Personnel"),
                Profile::new("professional", "Professionnel"),
            ],
        }
    }
}

impl Profiles {
    pub fn active(&self) -> Option<&Profile> {
        self.profiles
            .iter()
            .find(|p| p.id == self.active_profile_id)
    }

    pub fn validate(&self) -> Result<()> {
        validate_id(&self.active_profile_id)?;
        if self.profiles.is_empty() || self.profiles.len() > 16 || self.active().is_none() {
            return Err(RemoteError::InvalidConfiguration);
        }
        let mut ids = HashSet::new();
        for profile in &self.profiles {
            profile.validate()?;
            if !ids.insert(&profile.id) {
                return Err(RemoteError::InvalidConfiguration);
            }
        }
        Ok(())
    }
}

pub(crate) fn validate_id(id: &str) -> Result<()> {
    if id.is_empty()
        || id.len() > 64
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        Err(RemoteError::InvalidConfiguration)
    } else {
        Ok(())
    }
}

fn validate_text(value: &str, max: usize) -> Result<()> {
    if value.trim().is_empty() || value.len() > max || value.chars().any(char::is_control) {
        Err(RemoteError::InvalidConfiguration)
    } else {
        Ok(())
    }
}
