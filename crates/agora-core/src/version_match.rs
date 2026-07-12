//! Version comparison and range matching for JAR-declared incompatibilities.
//!
//! Fabric mods use predicate strings like `">=0.40.0"`, `"<2.0"`, `"*"`, with
//! space-separated AND-joined sub-predicates. Arrays of predicate strings are
//! OR-joined. Forge/NeoForge uses Maven-style ranges like `"[1.0,2.0)"`.
//!
//! Minecraft mod versions are frequently non-SemVer (e.g. `MC1.20.1-3.2.1-build.42`,
//! `0.5.3+build.2`), so the comparator is intentionally lenient: it splits on
//! `.`, `-`, `+`, `_` and compares segment-by-segment, numerically when both
//! segments are numeric, lexicographically otherwise.

use std::cmp::Ordering;

/// Result of evaluating a version-range declaration against an installed version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionMatch {
    /// The installed version falls within the declared incompatible range.
    Matched,
    /// The installed version is outside the declared range.
    NotMatched,
    /// The declaration has no version constraint (unconditional — matches any).
    Unconditional,
}

// ---------------------------------------------------------------------------
// Version comparison
// ---------------------------------------------------------------------------

/// Compare two version strings leniently. Splits on `.`, `-`, `+`, `_` and
/// compares segment-by-segment. Numeric segments compare numerically; when
/// one segment is numeric and the other is not, the numeric segment is
/// considered "greater" (newer). Non-numeric segments compare lexicographically.
/// When one version is a prefix of the other, the longer one is "greater"
/// (unless all remaining segments are zero/empty, in which case they're equal).
pub fn compare_versions(a: &str, b: &str) -> Ordering {
    let seg_a = split_version_segments(a);
    let seg_b = split_version_segments(b);
    let max = seg_a.len().max(seg_b.len());
    for i in 0..max {
        let sa = *seg_a.get(i).unwrap_or(&"");
        let sb = *seg_b.get(i).unwrap_or(&"");
        let ord = compare_segments(sa, sb);
        if ord != Ordering::Equal {
            return ord;
        }
    }
    Ordering::Equal
}

/// Split a version string into segments on `.`, `-`, `+`, `_`.
fn split_version_segments(v: &str) -> Vec<&str> {
    v.split(['.', '-', '+', '_']).collect()
}

/// Compare two version segments. If both parse as integers, compare numerically.
/// If one is numeric and the other isn't, numeric > non-numeric (so `2` > `beta`).
/// If neither is numeric, compare lexicographically (case-insensitive).
/// An empty segment is treated as `0` so that `1.0.0` equals `1.0`.
fn compare_segments(a: &str, b: &str) -> Ordering {
    let na = if a.is_empty() {
        Some(0)
    } else {
        a.parse::<i64>().ok()
    };
    let nb = if b.is_empty() {
        Some(0)
    } else {
        b.parse::<i64>().ok()
    };
    match (na, nb) {
        (Some(x), Some(y)) => x.cmp(&y),
        (Some(_), None) => Ordering::Greater,
        (None, Some(_)) => Ordering::Less,
        (None, None) => a.to_lowercase().cmp(&b.to_lowercase()),
    }
}

// ---------------------------------------------------------------------------
// Fabric predicate matching
// ---------------------------------------------------------------------------

/// A single comparison operator parsed from a Fabric predicate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CmpOp {
    Greater,
    GreaterEqual,
    Less,
    LessEqual,
    Equal,
    Any,
}

/// Evaluate a Fabric predicate string against an installed version.
///
/// A predicate string like `">=1.0 <2.0"` contains space-separated
/// sub-predicates that are AND-joined. Returns `true` only if ALL
/// sub-predicates match.
///
/// Recognized operators: `>=`, `<=`, `>`, `<`, `=`, `~` (approximate —
/// treats as `>=` for now), and `*` (any version). A bare version with no
/// operator is treated as exact match.
pub fn fabric_predicate_matches(predicate: &str, version: &str) -> bool {
    let trimmed = predicate.trim();
    if trimmed.is_empty() || trimmed == "*" {
        return true;
    }
    for sub in trimmed.split_whitespace() {
        if !fabric_single_matches(sub, version) {
            return false;
        }
    }
    true
}

fn fabric_single_matches(sub: &str, version: &str) -> bool {
    if sub == "*" {
        return true;
    }
    let (op, ver) = parse_predicate_operator(sub);
    match op {
        CmpOp::Any => {
            // No operator — treat as exact match (Fabric spec: bare version = exact).
            compare_versions(version, ver) == Ordering::Equal
        }
        CmpOp::Greater => compare_versions(version, ver) == Ordering::Greater,
        CmpOp::GreaterEqual => {
            matches!(
                compare_versions(version, ver),
                Ordering::Greater | Ordering::Equal
            )
        }
        CmpOp::Less => compare_versions(version, ver) == Ordering::Less,
        CmpOp::LessEqual => {
            matches!(
                compare_versions(version, ver),
                Ordering::Less | Ordering::Equal
            )
        }
        CmpOp::Equal => compare_versions(version, ver) == Ordering::Equal,
    }
}

/// Parse the operator prefix from a predicate like `">=1.0"` → `(GreaterEqual, "1.0")`.
fn parse_predicate_operator(s: &str) -> (CmpOp, &str) {
    for (prefix, op) in [
        (">=", CmpOp::GreaterEqual),
        ("<=", CmpOp::LessEqual),
        (">", CmpOp::Greater),
        ("<", CmpOp::Less),
        ("=", CmpOp::Equal),
        ("~", CmpOp::GreaterEqual), // approximate → treat as >=
    ] {
        if let Some(rest) = s.strip_prefix(prefix) {
            return (op, rest.trim());
        }
    }
    (CmpOp::Any, s)
}

/// Evaluate a list of Fabric predicate strings against an installed version.
///
/// The list has OR semantics: if ANY entry matches, the whole declaration
/// matches. Each entry is itself an AND of space-separated sub-predicates.
/// An empty list means unconditional (any version matches).
pub fn fabric_ranges_match(ranges: &[String], version: &str) -> bool {
    if ranges.is_empty() {
        return true;
    }
    ranges.iter().any(|r| fabric_predicate_matches(r, version))
}

// ---------------------------------------------------------------------------
// Forge Maven range matching
// ---------------------------------------------------------------------------

/// Evaluate a Forge/NeoForge Maven version range against an installed version.
///
/// Maven range grammar:
/// - `[a,b)` — `>= a, < b` (inclusive lower, exclusive upper)
/// - `[a,b]` — `>= a, <= b` (inclusive both)
/// - `(a,b)` — `> a, < b` (exclusive both)
/// - `(a,b]` — `> a, <= b` (exclusive lower, inclusive upper)
/// - `[a,]` or `[a,)` — `>= a` (no upper bound)
/// - `(,b]` or `(,b)` — `< b` or `<= b` (no lower bound)
/// - `[a]` — exact match `== a`
/// - bare `a` (no brackets) — `>= a` (Maven treats bare version as minimum)
pub fn maven_range_matches(range: &str, version: &str) -> bool {
    let trimmed = range.trim();
    if trimmed.is_empty() || trimmed == "*" {
        return true;
    }

    // Bracketed range: [a,b) / (a,b] / [a,b] / (a,b) / [a,] / (,b] / [a] etc.
    if let Some(inner) = parse_maven_brackets(trimmed) {
        return inner.matches(version);
    }

    // Bare version (no brackets): Maven treats as minimum (>= version).
    compare_versions(version, trimmed) != Ordering::Less
}

struct MavenBracketRange {
    lower_inclusive: bool,
    upper_inclusive: bool,
    lower: Option<String>,
    upper: Option<String>,
}

impl MavenBracketRange {
    fn matches(&self, version: &str) -> bool {
        if let Some(ref lower) = self.lower {
            let cmp = compare_versions(version, lower);
            if self.lower_inclusive {
                if cmp == Ordering::Less {
                    return false;
                }
            } else if cmp != Ordering::Greater {
                return false;
            }
        }
        if let Some(ref upper) = self.upper {
            let cmp = compare_versions(version, upper);
            if self.upper_inclusive {
                if cmp == Ordering::Greater {
                    return false;
                }
            } else if cmp != Ordering::Less {
                return false;
            }
        }
        true
    }
}

fn parse_maven_brackets(s: &str) -> Option<MavenBracketRange> {
    let bytes = s.as_bytes();
    if bytes.is_empty() || (bytes[0] != b'[' && bytes[0] != b'(') {
        return None;
    }
    let lower_inclusive = bytes[0] == b'[';
    let last = *bytes.last()?;
    if last != b']' && last != b')' {
        return None;
    }
    let upper_inclusive = last == b']';
    let inner = &s[1..s.len() - 1];
    let parts: Vec<&str> = inner.splitn(2, ',').collect();

    match parts.as_slice() {
        [lower, upper] => {
            let lower = lower.trim();
            let upper = upper.trim();
            let lower = if lower.is_empty() {
                None
            } else {
                Some(lower.to_string())
            };
            let upper = if upper.is_empty() {
                None
            } else {
                Some(upper.to_string())
            };
            Some(MavenBracketRange {
                lower_inclusive,
                upper_inclusive,
                lower,
                upper,
            })
        }
        [single] => {
            // [a] — exact match
            let v = single.trim();
            if v.is_empty() {
                return None;
            }
            Some(MavenBracketRange {
                lower_inclusive: true,
                upper_inclusive: true,
                lower: Some(v.to_string()),
                upper: Some(v.to_string()),
            })
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Unified evaluation
// ---------------------------------------------------------------------------

/// Evaluate whether an incompatibility declaration's version ranges match
/// the installed target version. This is the entry point used by the health
/// check.
///
/// - `version_ranges`: the predicates/ranges from the `IncompatibilityDecl`.
/// - `target_version`: the installed target mod's version string.
/// - `is_fabric_grammar`: true for Fabric/Quilt predicates, false for Forge Maven ranges.
pub fn evaluate_version_match(
    version_ranges: &[String],
    target_version: &str,
    is_fabric_grammar: bool,
) -> VersionMatch {
    if version_ranges.is_empty()
        || version_ranges
            .iter()
            .any(|r| r.trim() == "*" || r.trim().is_empty())
    {
        return VersionMatch::Unconditional;
    }
    let matched = if is_fabric_grammar {
        fabric_ranges_match(version_ranges, target_version)
    } else {
        version_ranges
            .iter()
            .any(|r| maven_range_matches(r, target_version))
    };
    if matched {
        VersionMatch::Matched
    } else {
        VersionMatch::NotMatched
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- compare_versions ---

    #[test]
    fn compare_numeric_versions() {
        assert_eq!(compare_versions("1.0", "1.0"), Ordering::Equal);
        assert_eq!(compare_versions("2.0", "1.0"), Ordering::Greater);
        assert_eq!(compare_versions("1.0", "2.0"), Ordering::Less);
        assert_eq!(compare_versions("1.10", "1.9"), Ordering::Greater);
        assert_eq!(compare_versions("1.0.0", "1.0"), Ordering::Equal);
    }

    #[test]
    fn compare_non_semver_versions() {
        assert_eq!(
            compare_versions("MC1.20.1-3.2.1", "MC1.20.1-3.2.0"),
            Ordering::Greater
        );
        assert_eq!(
            compare_versions("0.5.3+build.2", "0.5.3+build.1"),
            Ordering::Greater
        );
        assert_eq!(compare_versions("1.0.0", "1.0.0"), Ordering::Equal);
    }

    #[test]
    fn compare_mixed_numeric_string() {
        // Numeric > non-numeric segment
        assert_eq!(compare_versions("2.0", "beta"), Ordering::Greater);
        assert_eq!(compare_versions("1.0.0", "1.0.beta"), Ordering::Greater);
    }

    // --- Fabric predicate matching ---

    #[test]
    fn fabric_predicate_exact() {
        assert!(fabric_predicate_matches("1.0", "1.0"));
        assert!(!fabric_predicate_matches("1.0", "1.1"));
    }

    #[test]
    fn fabric_predicate_less_than() {
        assert!(fabric_predicate_matches("<2.0", "1.9"));
        assert!(!fabric_predicate_matches("<2.0", "2.0"));
        assert!(!fabric_predicate_matches("<2.0", "2.1"));
    }

    #[test]
    fn fabric_predicate_greater_equal() {
        assert!(fabric_predicate_matches(">=1.0", "1.0"));
        assert!(fabric_predicate_matches(">=1.0", "2.0"));
        assert!(!fabric_predicate_matches(">=1.0", "0.9"));
    }

    #[test]
    fn fabric_predicate_and() {
        // Space-separated = AND
        assert!(fabric_predicate_matches(">=1.0 <2.0", "1.5"));
        assert!(!fabric_predicate_matches(">=1.0 <2.0", "0.5")); // < lower
        assert!(!fabric_predicate_matches(">=1.0 <2.0", "2.5")); // > upper
        assert!(fabric_predicate_matches(">=1.0 <2.0", "1.0")); // boundary
    }

    #[test]
    fn fabric_predicate_wildcard() {
        assert!(fabric_predicate_matches("*", "anything"));
        assert!(fabric_predicate_matches("*", "1.0"));
    }

    // --- Fabric ranges (OR) ---

    #[test]
    fn fabric_ranges_or_semantics() {
        let ranges = vec!["<2.0".to_string(), ">=3.0".to_string()];
        assert!(fabric_ranges_match(&ranges, "1.5")); // matches <2.0
        assert!(fabric_ranges_match(&ranges, "3.5")); // matches >=3.0
        assert!(!fabric_ranges_match(&ranges, "2.5")); // matches neither
    }

    #[test]
    fn fabric_ranges_empty_is_match() {
        assert!(fabric_ranges_match(&[], "anything"));
    }

    // --- Forge Maven range matching ---

    #[test]
    fn maven_range_inclusive_exclusive() {
        assert!(maven_range_matches("[1.0,2.0)", "1.0"));
        assert!(maven_range_matches("[1.0,2.0)", "1.5"));
        assert!(!maven_range_matches("[1.0,2.0)", "2.0")); // exclusive upper
        assert!(!maven_range_matches("[1.0,2.0)", "0.9"));
    }

    #[test]
    fn maven_range_inclusive_both() {
        assert!(maven_range_matches("[1.0,2.0]", "1.0"));
        assert!(maven_range_matches("[1.0,2.0]", "2.0"));
        assert!(!maven_range_matches("[1.0,2.0]", "0.9"));
        assert!(!maven_range_matches("[1.0,2.0]", "2.1"));
    }

    #[test]
    fn maven_range_exact() {
        assert!(maven_range_matches("[1.0]", "1.0"));
        assert!(!maven_range_matches("[1.0]", "1.1"));
    }

    #[test]
    fn maven_range_no_upper() {
        assert!(maven_range_matches("[1.0,)", "1.0"));
        assert!(maven_range_matches("[1.0,)", "99.0"));
        assert!(!maven_range_matches("[1.0,)", "0.9"));
        // Also [1.0,] form
        assert!(maven_range_matches("[1.0,]", "5.0"));
    }

    #[test]
    fn maven_range_no_lower() {
        assert!(maven_range_matches("(,2.0]", "1.0"));
        assert!(maven_range_matches("(,2.0]", "2.0"));
        assert!(!maven_range_matches("(,2.0]", "2.1"));
    }

    #[test]
    fn maven_range_bare_version() {
        // Bare version = >= minimum
        assert!(maven_range_matches("1.0", "1.0"));
        assert!(maven_range_matches("1.0", "2.0"));
        assert!(!maven_range_matches("1.0", "0.9"));
    }

    #[test]
    fn maven_range_wildcard() {
        assert!(maven_range_matches("*", "anything"));
    }

    // --- Unified evaluate_version_match ---

    #[test]
    fn evaluate_fabric_matched() {
        let ranges = vec!["<2.0".to_string()];
        assert_eq!(
            evaluate_version_match(&ranges, "1.5", true),
            VersionMatch::Matched
        );
    }

    #[test]
    fn evaluate_fabric_not_matched() {
        let ranges = vec!["<2.0".to_string()];
        assert_eq!(
            evaluate_version_match(&ranges, "2.5", true),
            VersionMatch::NotMatched
        );
    }

    #[test]
    fn evaluate_fabric_unconditional() {
        let ranges: Vec<String> = vec![];
        assert_eq!(
            evaluate_version_match(&ranges, "anything", true),
            VersionMatch::Unconditional
        );
    }

    #[test]
    fn evaluate_forge_matched() {
        let ranges = vec!["[1.0,2.0)".to_string()];
        assert_eq!(
            evaluate_version_match(&ranges, "1.5", false),
            VersionMatch::Matched
        );
    }

    #[test]
    fn evaluate_forge_not_matched() {
        let ranges = vec!["[1.0,2.0)".to_string()];
        assert_eq!(
            evaluate_version_match(&ranges, "2.5", false),
            VersionMatch::NotMatched
        );
    }

    #[test]
    fn evaluate_non_semver_version() {
        let ranges = vec!["<2.0".to_string()];
        assert_eq!(
            evaluate_version_match(&ranges, "MC1.20.1-1.5", true),
            VersionMatch::Matched
        );
    }
}
