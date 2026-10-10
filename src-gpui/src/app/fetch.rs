//! Automatic fetch failures back off and stay hidden once dismissed, until
//! their summary changes or a fetch succeeds. Git output is summarized in one
//! French line; the raw text remains available as detail.
use crate::chrome::Notice;
use std::time::Duration;

const MAX_BACKOFF: Duration = Duration::from_secs(15 * 60);

#[derive(Default)]
pub struct FetchFeedback {
    failures: u32,
    dismissed: Option<String>,
}

impl FetchFeedback {
    /// Delay before the next automatic fetch: the interval, doubled after each
    /// consecutive failure up to 15 minutes, never shorter than the interval.
    pub fn delay(&self, interval_minutes: u32) -> Duration {
        let interval = Duration::from_secs(u64::from(interval_minutes) * 60);
        interval
            .saturating_mul(1 << self.failures.min(8))
            .min(MAX_BACKOFF.max(interval))
    }

    /// Records a finished fetch and returns the message to display. A manual
    /// fetch always shows its failure and re-arms a dismissed message.
    pub fn finish(&mut self, error: Option<String>, manual: bool) -> Option<Notice> {
        let Some(error) = error else {
            self.failures = 0;
            self.dismissed = None;
            return None;
        };
        self.failures = self.failures.saturating_add(1);
        let notice = Notice {
            summary: fetch_error_summary(&error),
            detail: Some(error),
        };
        if manual {
            self.dismissed = None;
        } else if self.dismissed.as_ref() == Some(&notice.summary) {
            return None;
        }
        Some(notice)
    }

    pub fn dismiss(&mut self, notice: Notice) {
        self.dismissed = Some(notice.summary);
    }

    /// Another repository starts with the configured interval.
    pub fn reset_backoff(&mut self) {
        self.failures = 0;
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Cause {
    HostNotFound,
    Timeout,
    Network,
    Authentication,
    RepositoryNotFound,
}

impl Cause {
    // Checked in order: SSH failures always end with a generic access-rights
    // hint, so the first specific diagnostic wins.
    const PATTERNS: [(Self, &'static [&'static str]); 5] = [
        (
            Self::HostNotFound,
            &[
                "could not resolve host",
                "name or service not known",
                "nodename nor servname",
                "temporary failure in name resolution",
                "no such host",
            ],
        ),
        (Self::Timeout, &["timed out", "timeout", "délai"]),
        (
            Self::Network,
            &[
                "network is unreachable",
                "network is down",
                "no route to host",
                "connection refused",
                "connection reset",
                "couldn't connect to server",
                "could not connect to server",
                "failed to connect to",
            ],
        ),
        (
            Self::Authentication,
            &[
                "permission denied",
                "authentication failed",
                "access denied",
                "could not read username",
                "could not read password",
                "terminal prompts disabled",
                "invalid username or password",
                "returned error: 401",
                "returned error: 403",
            ],
        ),
        (
            Self::RepositoryNotFound,
            &[
                "repository not found",
                "could not be found",
                "does not appear to be a git repository",
            ],
        ),
    ];

    fn detect(text: &str) -> Option<Self> {
        let text = text.to_lowercase();
        Self::PATTERNS
            .iter()
            .find(|(_, patterns)| patterns.iter().any(|pattern| text.contains(pattern)))
            .map(|(cause, _)| *cause)
    }

    fn label(self) -> &'static str {
        match self {
            Self::HostNotFound => "hôte introuvable",
            Self::Timeout => "délai dépassé",
            Self::Network => "réseau injoignable",
            Self::Authentication => "authentification refusée",
            Self::RepositoryNotFound => "dépôt distant introuvable",
        }
    }

    fn hint(self) -> &'static str {
        match self {
            Self::HostNotFound => "Vérifiez la connexion, le VPN ou l’URL du remote.",
            Self::Timeout => "Le serveur ne répond pas, réessayez plus tard.",
            Self::Network => "Vérifiez la connexion ou le VPN.",
            Self::Authentication => "Vérifiez vos identifiants Git ou votre clé SSH.",
            Self::RepositoryNotFound => "Vérifiez l’URL du remote et vos droits d’accès.",
        }
    }
}

#[derive(PartialEq, Eq)]
enum Reason {
    Known(Cause),
    Output(String),
}

impl Reason {
    fn short(&self) -> &str {
        match self {
            Self::Known(cause) => cause.label(),
            Self::Output(line) => line,
        }
    }

    fn full(&self) -> String {
        match self {
            Self::Known(cause) => format!("{}. {}", cause.label(), cause.hint()),
            Self::Output(line) => line.clone(),
        }
    }
}

/// One French line for the error of a fetch. The core reports one
/// « remote : output » section per failing remote.
pub fn fetch_error_summary(raw: &str) -> String {
    let mut groups: Vec<(Reason, Vec<&str>)> = Vec::new();
    for (remote, text) in remote_sections(raw) {
        let reason =
            Cause::detect(&text).map_or_else(|| Reason::Output(first_line(&text)), Reason::Known);
        match groups.iter_mut().find(|(existing, _)| *existing == reason) {
            Some((_, remotes)) => remotes.extend(remote),
            None => groups.push((reason, remote.into_iter().collect())),
        }
    }
    match groups.as_slice() {
        [] => "Fetch impossible.".into(),
        [(reason, remotes)] if remotes.is_empty() => {
            format!("Fetch impossible : {}", reason.full())
        }
        [(reason, remotes)] => {
            format!(
                "Fetch impossible ({}) : {}",
                remotes.join(", "),
                reason.full()
            )
        }
        groups => format!(
            "Fetch impossible : {}",
            groups
                .iter()
                .map(|(reason, remotes)| {
                    if remotes.is_empty() {
                        reason.short().to_owned()
                    } else {
                        format!("{} ({})", remotes.join(", "), reason.short())
                    }
                })
                .collect::<Vec<_>>()
                .join(" ; ")
        ),
    }
}

fn remote_sections(raw: &str) -> Vec<(Option<&str>, String)> {
    let mut sections: Vec<(Option<&str>, String)> = Vec::new();
    for line in raw.lines() {
        let remote = line
            .split_once(" : ")
            .filter(|(name, _)| !name.is_empty() && !name.contains(char::is_whitespace));
        match (remote, sections.last_mut()) {
            (Some((name, rest)), _) => sections.push((Some(name), rest.to_owned())),
            (None, Some((_, text))) => {
                text.push('\n');
                text.push_str(line);
            }
            (None, None) => sections.push((None, line.to_owned())),
        }
    }
    sections
}

fn first_line(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    ["fatal: ", "error: ", "ssh: "]
        .iter()
        .find_map(|prefix| line.strip_prefix(prefix))
        .unwrap_or(line)
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const UNREACHABLE: &str = "origin : fatal: unable to access 'https://example.com/r.git/': \
        Failed to connect to example.com port 443 after 12 ms: Couldn't connect to server";

    #[test]
    fn summarizes_common_git_failures_in_french() {
        let ssh_host = "origin : ssh: Could not resolve hostname example.com: \
            nodename nor servname provided, or not known\n\
            fatal: Could not read from remote repository.\n\n\
            Please make sure you have the correct access rights\nand the repository exists.";
        assert_eq!(
            fetch_error_summary(ssh_host),
            "Fetch impossible (origin) : hôte introuvable. Vérifiez la connexion, le VPN ou l’URL du remote."
        );
        assert_eq!(
            fetch_error_summary(UNREACHABLE),
            "Fetch impossible (origin) : réseau injoignable. Vérifiez la connexion ou le VPN."
        );
        let denied = "origin : git@example.com: Permission denied (publickey).\n\
            fatal: Could not read from remote repository.";
        assert!(fetch_error_summary(denied).contains("authentification refusée"));
        let prompt = "origin : fatal: could not read Username for 'https://example.com': \
            terminal prompts disabled";
        assert!(fetch_error_summary(prompt).contains("authentification refusée"));
        let timeout = "origin : ssh: connect to host example.com port 22: Operation timed out";
        assert!(fetch_error_summary(timeout).contains("délai dépassé"));
        assert!(
            fetch_error_summary("origin : Le fetch a dépassé le délai de deux minutes.")
                .contains("délai dépassé")
        );
        let missing = "origin : remote: Repository not found.\n\
            fatal: repository 'https://example.com/r.git/' not found";
        assert!(fetch_error_summary(missing).contains("dépôt distant introuvable"));
    }

    #[test]
    fn groups_remotes_and_keeps_unknown_output_on_one_line() {
        let same = format!(
            "{UNREACHABLE}\n{}",
            UNREACHABLE.replace("origin", "upstream")
        );
        assert_eq!(
            fetch_error_summary(&same),
            "Fetch impossible (origin, upstream) : réseau injoignable. Vérifiez la connexion ou le VPN."
        );
        let mixed = format!(
            "{UNREACHABLE}\nfork : fatal: Authentication failed for 'https://example.com/'"
        );
        assert_eq!(
            fetch_error_summary(&mixed),
            "Fetch impossible : origin (réseau injoignable) ; fork (authentification refusée)"
        );
        assert_eq!(
            fetch_error_summary("origin : fatal: unexpected disconnect\nmore output"),
            "Fetch impossible (origin) : unexpected disconnect"
        );
        assert_eq!(
            fetch_error_summary("Une récupération des références distantes est déjà en cours."),
            "Fetch impossible : Une récupération des références distantes est déjà en cours."
        );
        let notice = FetchFeedback::default()
            .finish(Some(UNREACHABLE.into()), false)
            .unwrap();
        assert_eq!(notice.detail.as_deref(), Some(UNREACHABLE));
        assert_eq!(notice.summary, fetch_error_summary(UNREACHABLE));
    }

    #[test]
    fn dismissed_error_stays_hidden_until_it_changes_or_a_fetch_succeeds() {
        let mut feedback = FetchFeedback::default();
        let shown = feedback.finish(Some(UNREACHABLE.into()), false).unwrap();
        feedback.dismiss(shown.clone());
        // Git varies timings in its output; the summary stays the same.
        let retry = UNREACHABLE.replace("12 ms", "75003 ms");
        assert_eq!(feedback.finish(Some(retry), false), None);
        let changed = "origin : fatal: Authentication failed".to_owned();
        assert_eq!(
            feedback
                .finish(Some(changed.clone()), false)
                .unwrap()
                .detail,
            Some(changed)
        );
        feedback.dismiss(shown.clone());
        assert_eq!(feedback.finish(None, false), None);
        assert_eq!(
            feedback.finish(Some(UNREACHABLE.into()), false),
            Some(shown)
        );
    }

    #[test]
    fn manual_fetch_always_reports_and_rearms_the_message() {
        let mut feedback = FetchFeedback::default();
        let shown = feedback.finish(Some(UNREACHABLE.into()), false).unwrap();
        feedback.dismiss(shown.clone());
        assert_eq!(
            feedback.finish(Some(UNREACHABLE.into()), true),
            Some(shown.clone())
        );
        assert_eq!(
            feedback.finish(Some(UNREACHABLE.into()), false),
            Some(shown)
        );
    }

    #[test]
    fn failures_double_the_delay_up_to_fifteen_minutes_then_reset() {
        let minutes = |minutes: u64| Duration::from_secs(minutes * 60);
        let mut feedback = FetchFeedback::default();
        assert_eq!(feedback.delay(1), minutes(1));
        let mut delays = Vec::new();
        for _ in 0..6 {
            feedback.finish(Some(UNREACHABLE.into()), false);
            delays.push(feedback.delay(1));
        }
        assert_eq!(
            delays,
            [2, 4, 8, 15, 15, 15].map(minutes),
            "auto-fetch backs off after consecutive failures"
        );
        assert_eq!(feedback.delay(30), minutes(30));
        feedback.finish(None, false);
        assert_eq!(feedback.delay(1), minutes(1));
        feedback.finish(Some(UNREACHABLE.into()), true);
        feedback.reset_backoff();
        assert_eq!(feedback.delay(5), minutes(5));
    }
}
