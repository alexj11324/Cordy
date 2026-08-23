//! VCS provider abstraction (port of server/internal/integrations/vcs):
//! Forgejo/Gitea and GitLab webhook + token adapters behind one
//! normalized PR/CI event shape.

pub mod forgejo;
pub mod gitlab;
pub mod vcs;

pub use forgejo::ForgejoProvider;
pub use gitlab::GitlabProvider;
pub use vcs::{
    derive_pr_state, normalize_instance_url, Account, CiStatusEvent, EventKind, Kind, Provider,
    ProviderError, PullRequestEvent, UnauthorizedError,
};

/// The registry mapping a kind string to its Provider. Go populates it via
/// package init in the adapter files; Rust names the constructors here —
/// the only user-visible difference per provider is its label.
pub fn for_kind(kind: &str) -> Option<Box<dyn Provider>> {
    match kind {
        "forgejo" => Some(Box::new(ForgejoProvider::new(Kind::FORGEJO))),
        "gitea" => Some(Box::new(ForgejoProvider::new(Kind::GITEA))),
        "gitlab" => Some(Box::new(GitlabProvider)),
        _ => None,
    }
}
