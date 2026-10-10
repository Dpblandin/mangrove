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
    OAuthPortInUse,
    OAuthAccessDenied,
    OAuthDeviceFlowDisabled,
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
    CloneDestinationExists,
    CloneDestinationInvalid,
    CloneAuthentication,
    CloneFailed,
}

impl fmt::Display for RemoteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::Cancelled => "Lecture annulée.",
            Self::InvalidConfiguration => "Configuration distante invalide.",
            Self::Network => "Connexion au serveur impossible.",
            Self::OAuthPortInUse => "Le port local de connexion est déjà utilisé. Terminez l’autre connexion puis réessayez.",
            Self::OAuthAccessDenied => "L’autorisation a été refusée dans le navigateur.",
            Self::OAuthDeviceFlowDisabled => "Activez Enable Device Flow dans les paramètres de l’application GitHub.",
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
            Self::CloneDestinationExists => "Ce dossier existe déjà. Choisissez un nouveau dossier pour le clonage.",
            Self::CloneDestinationInvalid => "Choisissez un nouveau dossier dans un emplacement existant et accessible.",
            Self::CloneAuthentication => "Git ne dispose pas des identifiants nécessaires. Configurez votre gestionnaire Git ou une clé SSH.",
            Self::CloneFailed => "Le clonage a échoué. Vérifiez l’URL et vos accès Git. Un dossier incomplet peut subsister.",
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

/// Only public OAuth configuration is stored alongside account metadata.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Authentication {
    #[default]
    PersonalToken,
    GitLabOAuth {
        client_id: String,
    },
    GitHubOAuth {
        client_id: String,
    },
    BitbucketOAuth {
        client_id: String,
    },
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Account {
    pub id: String,
    pub label: String,
    pub provider: Provider,
    pub server_url: String,
    pub username: Option<String>,
    #[serde(default)]
    pub authentication: Authentication,
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
        match &self.authentication {
            Authentication::PersonalToken => {}
            Authentication::GitLabOAuth { client_id } => {
                if self.provider != Provider::GitLab {
                    return Err(RemoteError::InvalidConfiguration);
                }
                crate::oauth::validate_client_id(client_id)?;
            }
            Authentication::GitHubOAuth { client_id } => {
                if self.provider != Provider::GitHub {
                    return Err(RemoteError::InvalidConfiguration);
                }
                crate::oauth::validate_client_id(client_id)?;
            }
            Authentication::BitbucketOAuth { client_id } => {
                if self.provider != Provider::BitbucketCloud {
                    return Err(RemoteError::InvalidConfiguration);
                }
                crate::oauth::validate_client_id(client_id)?;
                crate::bitbucket_oauth::cloud_workspace(&self.server_url)?;
            }
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
    /// The longest accepted name, in characters.
    pub const MAX_NAME_CHARS: usize = 128;

    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            accounts: Vec::new(),
            favorites: Vec::new(),
            ssh: SshChoice::default(),
        }
    }

    /// Checks a profile name with the rules applied to persisted names.
    pub fn validate_name(name: &str) -> Result<()> {
        validate_text(name, Self::MAX_NAME_CHARS)
    }

    pub fn validate(&self) -> Result<()> {
        validate_id(&self.id)?;
        Self::validate_name(&self.name)?;
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

    /// Renames a profile with the rules applied to persisted names.
    pub fn rename(&mut self, id: &str, name: &str) -> Result<()> {
        let name = name.trim();
        Profile::validate_name(name)?;
        let profile = self
            .profiles
            .iter_mut()
            .find(|profile| profile.id == id)
            .ok_or(RemoteError::InvalidConfiguration)?;
        profile.name = name.to_string();
        Ok(())
    }

    /// Removes an inactive profile. The active profile and the last profile
    /// are always kept.
    pub fn remove(&mut self, id: &str) -> Result<()> {
        if id == self.active_profile_id || self.profiles.len() < 2 {
            return Err(RemoteError::InvalidConfiguration);
        }
        let index = self
            .profiles
            .iter()
            .position(|profile| profile.id == id)
            .ok_or(RemoteError::InvalidConfiguration)?;
        self.profiles.remove(index);
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

fn validate_text(value: &str, max_chars: usize) -> Result<()> {
    if value.trim().is_empty()
        || value.chars().count() > max_chars
        || value.chars().any(char::is_control)
    {
        Err(RemoteError::InvalidConfiguration)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renaming_trims_and_validates_the_name_of_an_existing_profile() {
        let mut profiles = Profiles::default();
        // The limit counts characters, not UTF-8 bytes.
        profiles.rename("professional", &"é".repeat(128)).unwrap();
        profiles.rename("professional", "  Client Acme ").unwrap();
        assert_eq!(profiles.profiles[1].name, "Client Acme");
        for invalid in ["", "   ", "line\nfeed", "tab\tname", &"é".repeat(129)] {
            assert_eq!(
                profiles.rename("professional", invalid),
                Err(RemoteError::InvalidConfiguration)
            );
        }
        assert_eq!(
            profiles.rename("missing", "Nom"),
            Err(RemoteError::InvalidConfiguration)
        );
        assert_eq!(profiles.profiles[1].name, "Client Acme");
        profiles.validate().unwrap();
    }

    #[test]
    fn removing_keeps_the_active_and_the_last_profile() {
        let mut profiles = Profiles::default();
        assert_eq!(
            profiles.remove("personal"),
            Err(RemoteError::InvalidConfiguration)
        );
        assert_eq!(
            profiles.remove("missing"),
            Err(RemoteError::InvalidConfiguration)
        );
        profiles.remove("professional").unwrap();
        assert_eq!(profiles.profiles.len(), 1);
        profiles.validate().unwrap();
        assert_eq!(
            profiles.remove("personal"),
            Err(RemoteError::InvalidConfiguration)
        );
    }
}
