use crate::{Error, Result};
use globset::GlobMatcher;
use std::{fs, path::Path};

/// One Syncthing ignore pattern.
#[derive(Debug, Clone)]
pub struct IgnoreRule {
    pub raw: String,
    pub negated: bool,
    pub dir_only: bool,
    matcher: GlobMatcher,
    basename_only: bool,
}

impl IgnoreRule {
    fn parse(line: &str) -> Result<Option<Self>> {
        let line = line.trim_end_matches('\r').trim();
        if line.is_empty() || line.starts_with('#') {
            return Ok(None);
        }
        let mut rest = line;
        let mut negated = false;
        if let Some(value) = rest.strip_prefix('!') {
            negated = true;
            rest = value;
        }
        let mut case_insensitive = false;
        loop {
            if let Some(value) = rest.strip_prefix("(?d)") {
                rest = value;
            } else if let Some(value) = rest.strip_prefix("(?i)") {
                case_insensitive = true;
                rest = value;
            } else {
                break;
            }
        }
        if rest.is_empty() {
            return Err(Error::InvalidProfile(format!("empty pattern: {line}")));
        }
        let dir_only = rest.ends_with('/');
        if dir_only {
            rest = rest.trim_end_matches('/');
        }
        let rooted = rest.starts_with('/');
        let rest = rest.strip_prefix('/').unwrap_or(rest);
        if rest.is_empty() {
            return Err(Error::InvalidProfile(format!("invalid pattern: {line}")));
        }
        let basename_only = !rooted && !rest.contains('/');
        let matcher = globset::GlobBuilder::new(rest)
            .literal_separator(true)
            .case_insensitive(case_insensitive)
            .backslash_escape(true)
            .build()
            .map_err(|e| Error::InvalidProfile(format!("bad pattern {line:?}: {e}")))?
            .compile_matcher();
        Ok(Some(Self {
            raw: line.to_string(),
            negated,
            dir_only,
            matcher,
            basename_only,
        }))
    }

    /// Deepest path level matched: 0 is the path itself, 1 its parent, ...
    fn depth(&self, path: &str, is_dir: bool) -> Option<usize> {
        let mut candidate = path.trim_matches('/');
        let mut depth = 0;
        loop {
            let candidate_is_dir = depth > 0 || is_dir;
            if !self.dir_only || candidate_is_dir {
                let subject = if self.basename_only {
                    candidate.rsplit('/').next().unwrap_or(candidate)
                } else {
                    candidate
                };
                if self.matcher.is_match(subject) {
                    return Some(depth);
                }
            }
            match candidate.rsplit_once('/') {
                Some((parent, _)) if !parent.is_empty() => {
                    candidate = parent;
                    depth += 1;
                }
                _ => return None,
            }
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct IgnoreSet {
    rules: Vec<IgnoreRule>,
}

impl IgnoreSet {
    pub fn parse<'a>(lines: impl IntoIterator<Item = &'a str>) -> Result<Self> {
        let mut rules = Vec::new();
        for line in lines {
            if let Some(rule) = IgnoreRule::parse(line)? {
                rules.push(rule);
            }
        }
        Ok(Self { rules })
    }

    pub fn from_text(text: &str) -> Result<Self> {
        Self::parse(text.lines())
    }

    pub fn from_file(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path)?;
        Self::from_text(&text)
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Last matching pattern wins. A pattern that matches a directory also
    /// applies to everything below it, which is how `*` plus `!/path` works.
    pub fn is_ignored(&self, path: &str, is_dir: bool) -> bool {
        let path = path.trim_matches('/');
        if path.is_empty() {
            return false;
        }
        let mut ignored = false;
        for rule in &self.rules {
            if rule.depth(path, is_dir).is_some() {
                ignored = !rule.negated;
            }
        }
        ignored
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::escape_glob;

    #[test]
    fn ignore_then_allow_list() {
        let set = IgnoreSet::from_text(
            "*\n!/originals/Years/2024/2024.05.01 Coast\n!/published/Years/2024/2024.05.01 Coast\n",
        )
        .unwrap();
        assert!(!set.is_ignored("originals/Years/2024/2024.05.01 Coast/a.jpg", false));
        assert!(!set.is_ignored("published/Years/2024/2024.05.01 Coast/a.jpg", false));
        assert!(set.is_ignored("originals/Years/2024/Other/a.jpg", false));
        assert!(set.is_ignored("inbox/PixelMob/a.jpg", false));
    }

    #[test]
    fn directory_pattern_ignores_children() {
        let set = IgnoreSet::from_text("(?d)C1 caches\n(?d)**/.DS_Store\n").unwrap();
        assert!(set.is_ignored("C1 caches/x", false));
        assert!(set.is_ignored("a/C1 caches/x", false));
        assert!(set.is_ignored("a/.DS_Store", false));
        assert!(!set.is_ignored("a/photo.jpg", false));
    }

    #[test]
    fn escaped_project_names_match_literally() {
        let path = "originals/Projects/Trip [2024] {a}?*";
        let set = IgnoreSet::from_text(&format!("*\n!/{}\n", escape_glob(path))).unwrap();
        assert!(!set.is_ignored(&format!("{path}/a.jpg"), false));
        assert!(set.is_ignored("originals/Projects/Trip 2024/a.jpg", false));
    }

    #[test]
    fn case_insensitive_prefix() {
        let set = IgnoreSet::from_text("(?i)(?d)cache\n").unwrap();
        assert!(set.is_ignored("Cache/x", false));
    }

    #[test]
    fn root_anchor_and_dir_only() {
        let set = IgnoreSet::from_text("/trash/\n").unwrap();
        assert!(set.is_ignored("trash", true));
        assert!(!set.is_ignored("a/trash", true));
        assert!(!set.is_ignored("trash", false));
    }

    #[test]
    fn comments_and_includes_are_not_rules() {
        let set = IgnoreSet::from_text("# comment\n#include other.txt\n\n(?d).DS_Store\n").unwrap();
        assert_eq!(set.rules.len(), 1);
    }
}
