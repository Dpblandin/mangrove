use std::{fmt, str::FromStr};

/// A release number, `X.Y.Z`. Releases have no pre-release or build suffix.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

impl FromStr for Version {
    type Err = ();

    /// Accepts a tag such as `v0.4.0` as well as `0.4.0`.
    fn from_str(text: &str) -> Result<Self, ()> {
        let text = text.strip_prefix('v').unwrap_or(text);
        let mut parts = text.split('.').map(|part| {
            // `u64::from_str` would accept a leading `+`.
            if part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(());
            }
            part.parse().map_err(|_| ())
        });
        let version = Self {
            major: parts.next().ok_or(())??,
            minor: parts.next().ok_or(())??,
            patch: parts.next().ok_or(())??,
        };
        match parts.next() {
            Some(_) => Err(()),
            None => Ok(version),
        }
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

#[cfg(test)]
mod tests {
    use super::Version;

    fn version(text: &str) -> Version {
        text.parse().unwrap()
    }

    #[test]
    fn tags_and_versions_parse_and_compare_numerically() {
        assert_eq!(version("v0.3.0"), version("0.3.0"));
        assert_eq!(version("0.10.2").to_string(), "0.10.2");
        assert!(version("0.10.0") > version("0.9.12"));
        assert!(version("1.0.0") > version("0.99.99"));
        assert!(version("0.3.1") > version("0.3.0"));
    }

    #[test]
    fn anything_but_three_whole_numbers_is_refused() {
        for text in [
            "",
            "v",
            "0.3",
            "0.3.0.1",
            "0.3.0-beta",
            "0.3.+1",
            "0..1",
            "x.y.z",
            "vv0.3.0",
        ] {
            assert!(text.parse::<Version>().is_err(), "{text}");
        }
    }
}
