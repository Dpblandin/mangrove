//! Hosting, SSH discovery, and explicit cloning, independent of the local Git engine.
//!
//! Synchronous entry points belong on a background executor. Credentials never
//! appear in serializable models or diagnostics. Profile switches abort network
//! futures and invalidate the old profile's tickets and bounded memory cache.
//!
//! Secrets cannot be serialized into preferences or native response models:
//! ```compile_fail
//! use mangrove_remote::SecretToken;
//! let token = SecretToken::from("sensitive-fixture");
//! let _ = serde_json::to_string(&token);
//! ```

pub mod clone;
mod model;
pub mod oauth;
mod profiles;
pub mod providers;
mod session;
pub mod ssh;
mod transport;

pub use clone::clone_repository;
pub use model::{Account, Authentication, Profile, Profiles, Provider, RemoteError, Result};
pub use profiles::{CredentialVault, OsCredentialVault, ProfileStore};
pub use providers::{
    ChangeRequest, CiStatus, Comment, Discussion, Page, PageCursor, ProviderClient, Repository,
};
pub use secrecy::{ExposeSecret, SecretString as SecretToken};
pub use session::{CancellationToken, ProfileSession, SessionTicket};
pub use ssh::{read_references, RemoteReference, SshChoice};
pub use transport::{HttpRequest, HttpResponse, HttpTransport, SafeApiUrl, Transport};
