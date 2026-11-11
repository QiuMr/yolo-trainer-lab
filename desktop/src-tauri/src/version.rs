//! App version, injected by `build.rs` from `tauri.conf.json`.
//!
//! Do not add a second constant here. The Release tag, the NSIS filename, `latest.json` and this
//! value must all be the same string; `build.rs` is what makes that true by construction.

use std::cmp::Ordering;

/// The version compiled into this binary, e.g. `"0.1.0"`.
pub const VERSION: &str = env!("YOLO_TRAINER_VERSION");

/// Compare two `major.minor.patch` versions numerically.
///
/// Used by `updater::install()` as defence in depth: a feed copied from an older tag could
/// otherwise pair an old (validly signed) artifact with a newer version string. `check()` relies
/// on the plugin's own comparator; making it also explain "the feed is older" would need a second
/// HTTPS request with our own TLS stack, which is not worth a dependency for a misconfiguration
/// the user cannot fix from the UI anyway.
///
/// **Never compare version strings lexicographically**: `"0.10.0" < "0.9.0"` is true as text and
/// would make every release after 0.9.x look older. Missing segments count as 0; a leading `v`
/// (legal in the official feed format) and `-beta` / `+meta` suffixes are ignored.
pub fn cmp_version(a: &str, b: &str) -> Ordering {
    fn parts(s: &str) -> Vec<u64> {
        let core = s
            .trim()
            .trim_start_matches(['v', 'V'])
            .split(['-', '+'])
            .next()
            .unwrap_or("");
        core.split('.')
            .map(|part| part.trim().parse::<u64>().unwrap_or(0))
            .collect()
    }
    let (left, right) = (parts(a), parts(b));
    for i in 0..left.len().max(right.len()) {
        match left
            .get(i)
            .copied()
            .unwrap_or(0)
            .cmp(&right.get(i).copied().unwrap_or(0))
        {
            Ordering::Equal => continue,
            other => return other,
        }
    }
    Ordering::Equal
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering;

    #[test]
    fn injected_version_matches_the_config_format() {
        assert_eq!(VERSION.split('.').count(), 3, "VERSION = {VERSION}");
        assert!(VERSION.split('.').all(|p| !p.is_empty()));
    }

    /// The classic double-digit regression: lexicographic comparison gets this backwards.
    #[test]
    fn double_digit_minor_is_newer() {
        assert_eq!(cmp_version("0.10.0", "0.9.0"), Ordering::Greater);
        assert_eq!(cmp_version("0.9.0", "0.10.0"), Ordering::Less);
        assert_eq!(cmp_version("1.0.0", "0.99.99"), Ordering::Greater);
    }

    #[test]
    fn short_versions_and_suffixes() {
        assert_eq!(cmp_version("1.2", "1.2.0"), Ordering::Equal);
        assert_eq!(cmp_version("1.2.3-beta.1", "1.2.3"), Ordering::Equal);
        assert_eq!(cmp_version("1.2.3+build7", "1.2.4"), Ordering::Less);
        assert_eq!(cmp_version("garbage", "0.0.0"), Ordering::Equal);
    }

    /// The official updater feed allows a leading `v`; treating `v1` as 0 used to make a
    /// genuinely newer release look older and abort the install with a contradictory message.
    #[test]
    fn leading_v_is_legal() {
        assert_eq!(cmp_version("v1.0.0", "0.1.3"), Ordering::Greater);
        assert_eq!(cmp_version("V0.2.0", "v0.1.9"), Ordering::Greater);
        assert_eq!(cmp_version("v0.1.3", "0.1.3"), Ordering::Equal);
    }
}
