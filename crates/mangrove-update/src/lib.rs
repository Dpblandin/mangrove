//! Mangrove releases published on GitHub: finding the latest one, downloading
//! the archive of this platform, checking its minisign signature and putting
//! it in place of the running application.
//!
//! Every entry point blocks and belongs on a background executor. Nothing runs
//! on its own: the application decides when to check and when to install.

mod download;
mod http;
mod install;
mod release;
mod version;

pub use download::{download, Download, Progress};
pub use install::{Installation, NotInstallable};
pub use release::{latest_release, Asset, Platform, Release};
pub use url::Url;
pub use version::Version;

use std::{error, fmt, io};

pub type Result<T> = std::result::Result<T, UpdateError>;

#[derive(Debug)]
pub enum UpdateError {
    Cancelled,
    Network,
    Timeout,
    RateLimited,
    Http(u16),
    InvalidResponse,
    /// The download differs from the size GitHub announced.
    UnexpectedSize,
    /// The signature is missing, made by another key or for another file.
    InvalidSignature,
    /// This build carries no valid release key.
    MissingKey,
    /// The archive does not hold the expected application.
    InvalidArchive,
    ReadOnly,
    PermissionDenied,
    /// A system tool failed: its name and its last output line.
    Tool(&'static str, String),
    Io(io::Error),
}

impl fmt::Display for UpdateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled => f.write_str("Mise à jour annulée."),
            Self::Network => f.write_str("Impossible de joindre GitHub. Vérifiez la connexion."),
            Self::Timeout => f.write_str("GitHub n’a pas répondu dans le délai imparti."),
            Self::RateLimited => {
                f.write_str("GitHub limite les requêtes anonymes. Réessayez plus tard.")
            }
            Self::Http(status) => write!(f, "GitHub a refusé la requête (HTTP {status})."),
            Self::InvalidResponse => f.write_str("La réponse de GitHub n’est pas prise en charge."),
            Self::UnexpectedSize => {
                f.write_str("Le téléchargement n’a pas la taille annoncée par GitHub.")
            }
            Self::InvalidSignature => f.write_str(
                "La signature de l’archive est invalide : la mise à jour a été refusée.",
            ),
            Self::MissingKey => {
                f.write_str("Ce build ne contient pas la clé qui vérifie les mises à jour.")
            }
            Self::InvalidArchive => {
                f.write_str("L’archive téléchargée ne contient pas l’application attendue.")
            }
            Self::ReadOnly => f.write_str(
                "L’application est sur un volume en lecture seule. Déplacez-la avant de la mettre à jour.",
            ),
            Self::PermissionDenied => f.write_str(
                "Mangrove n’a pas le droit de remplacer l’application. Installez la mise à jour depuis sa page.",
            ),
            Self::Tool(tool, output) if output.is_empty() => write!(f, "{tool} a échoué."),
            Self::Tool(tool, output) => write!(f, "{tool} a échoué : {output}"),
            Self::Io(error) => write!(f, "Erreur de fichier : {error}"),
        }
    }
}

impl error::Error for UpdateError {}

impl From<io::Error> for UpdateError {
    fn from(error: io::Error) -> Self {
        match error.kind() {
            io::ErrorKind::PermissionDenied => Self::PermissionDenied,
            io::ErrorKind::ReadOnlyFilesystem => Self::ReadOnly,
            _ => Self::Io(error),
        }
    }
}

impl From<reqwest::Error> for UpdateError {
    fn from(error: reqwest::Error) -> Self {
        if error.is_timeout() {
            Self::Timeout
        } else if let Some(status) = error.status() {
            Self::Http(status.as_u16())
        } else if error.is_decode() {
            Self::InvalidResponse
        } else {
            Self::Network
        }
    }
}
