use crate::{normalize_relative, validate_name, Error, Result};
use chrono::{Datelike, NaiveDate};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const YEAR_ROOT: &str = "originals/Years";
pub const PROJECTS_ROOT: &str = "originals/Projects";
pub const PUBLISHED_ROOT: &str = "published";

/// Configurable library layout. Every field is a relative path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectLayout {
    pub originals: String,
    pub years: String,
    pub projects: String,
    pub published: String,
}

impl Default for ProjectLayout {
    fn default() -> Self {
        Self {
            originals: "originals".into(),
            years: "Years".into(),
            projects: "Projects".into(),
            published: "published".into(),
        }
    }
}

impl ProjectLayout {
    pub fn validate(&self) -> Result<()> {
        for value in [
            &self.originals,
            &self.years,
            &self.projects,
            &self.published,
        ] {
            normalize_relative(value)?;
        }
        Ok(())
    }

    pub fn year_prefix(&self) -> Result<String> {
        self.validate()?;
        Ok(format!("{}/{}", self.originals, self.years))
    }

    pub fn project_prefix(&self) -> Result<String> {
        self.validate()?;
        Ok(format!("{}/{}", self.originals, self.projects))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProjectKind {
    /// `originals/Years/<YYYY>/<name>`
    Year,
    /// `originals/Projects/<name>`
    Project,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub kind: ProjectKind,
    pub year: Option<i32>,
    pub name: String,
    /// Relative path of the originals directory.
    pub originals: String,
    /// Relative path of the optional published twin.
    pub published: String,
}

impl Project {
    pub fn from_originals(path: &str) -> Result<Self> {
        Self::from_originals_in(path, &ProjectLayout::default())
    }

    pub fn from_originals_in(path: &str, layout: &ProjectLayout) -> Result<Self> {
        let path = normalize_relative(path)?;
        let year_prefix = layout.year_prefix()?;
        let project_prefix = layout.project_prefix()?;
        if let Some(rest) = path.strip_prefix(&format!("{year_prefix}/")) {
            let parts: Vec<&str> = rest.split('/').collect();
            if let [year, name] = parts.as_slice() {
                let year: i32 = year
                    .parse()
                    .map_err(|_| Error::InvalidProject(format!("bad year in {path}")))?;
                if !(1000..=9999).contains(&year) {
                    return Err(Error::InvalidProject(format!("bad year in {path}")));
                }
                validate_name(name)?;
                return Ok(Self {
                    kind: ProjectKind::Year,
                    year: Some(year),
                    name: (*name).to_string(),
                    originals: path.clone(),
                    published: format!("{}/{}/{year}/{name}", layout.published, layout.years),
                });
            }
        }
        if let Some(rest) = path.strip_prefix(&format!("{project_prefix}/")) {
            let parts: Vec<&str> = rest.split('/').collect();
            if let [name] = parts.as_slice() {
                validate_name(name)?;
                return Ok(Self {
                    kind: ProjectKind::Project,
                    year: None,
                    name: (*name).to_string(),
                    originals: path.clone(),
                    published: format!("{}/{}/{name}", layout.published, layout.projects),
                });
            }
        }
        Err(Error::InvalidProject(format!(
            "{path} is not {year_prefix}/<YYYY>/<name> or {project_prefix}/<name>"
        )))
    }

    /// Build the year-project path used by "New project".
    pub fn new_year_project(date: NaiveDate, name: &str) -> Result<Self> {
        Self::new_year_project_in(date, name, &ProjectLayout::default())
    }

    pub fn new_year_project_in(
        date: NaiveDate,
        name: &str,
        layout: &ProjectLayout,
    ) -> Result<Self> {
        let name = name.trim();
        validate_name(name)?;
        let full = format!(
            "{}.{:02}.{:02} {name}",
            date.year(),
            date.month(),
            date.day()
        );
        Self::from_originals_in(
            &format!(
                "{}/{}/{}/{full}",
                layout.originals,
                layout.years,
                date.year()
            ),
            layout,
        )
    }

    pub fn paths(&self) -> [&str; 2] {
        [&self.originals, &self.published]
    }

    pub fn relative_to_published(&self) -> Option<&str> {
        Some(&self.published)
    }

    /// Derive candidate projects from file or directory paths in the hub tree.
    pub fn discover(paths: impl IntoIterator<Item = impl AsRef<str>>) -> Vec<Self> {
        Self::discover_in(paths, &ProjectLayout::default())
    }

    pub fn discover_in(
        paths: impl IntoIterator<Item = impl AsRef<str>>,
        layout: &ProjectLayout,
    ) -> Vec<Self> {
        let mut found: BTreeMap<String, Self> = BTreeMap::new();
        let year_prefix = layout
            .year_prefix()
            .unwrap_or_else(|_| YEAR_ROOT.to_string());
        let project_prefix = layout
            .project_prefix()
            .unwrap_or_else(|_| PROJECTS_ROOT.to_string());
        for raw in paths {
            let raw = raw.as_ref().trim_matches('/');
            let candidate = if let Some(rest) = raw.strip_prefix(&format!("{year_prefix}/")) {
                let parts: Vec<&str> = rest.split('/').collect();
                match parts.as_slice() {
                    [year, name, ..] => Some(format!("{year_prefix}/{year}/{name}")),
                    _ => None,
                }
            } else if let Some(rest) = raw.strip_prefix(&format!("{project_prefix}/")) {
                let parts: Vec<&str> = rest.split('/').collect();
                match parts.as_slice() {
                    [name, ..] => Some(format!("{project_prefix}/{name}")),
                    _ => None,
                }
            } else {
                None
            };
            if let Some(candidate) = candidate {
                if let Ok(project) = Self::from_originals_in(&candidate, layout) {
                    found.insert(project.originals.clone(), project);
                }
            }
        }
        found.into_values().collect()
    }

    pub fn size_key(&self) -> &str {
        &self.originals
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_both_project_layouts() {
        let year = Project::from_originals("originals/Years/2024/2024.05.01 Coast").unwrap();
        assert_eq!(year.kind, ProjectKind::Year);
        assert_eq!(year.year, Some(2024));
        assert_eq!(year.name, "2024.05.01 Coast");
        assert_eq!(year.published, "published/Years/2024/2024.05.01 Coast");

        let project = Project::from_originals("originals/Projects/Family").unwrap();
        assert_eq!(project.kind, ProjectKind::Project);
        assert_eq!(project.year, None);
        assert_eq!(project.published, "published/Projects/Family");
    }

    #[test]
    fn rejects_traversal_and_bad_layouts() {
        for path in [
            "originals/Years/2024/../Secret",
            "/originals/Years/2024/A",
            "originals/Years/24/A",
            "originals/Years/2024/A/B",
            "originals/Projects/",
            "published/Years/2024/A",
        ] {
            assert!(Project::from_originals(path).is_err(), "{path}");
        }
    }

    #[test]
    fn discovers_projects_from_tree_paths() {
        let projects = Project::discover([
            "originals/Years/2024/2024.05.01 Coast/DSC1.jpg",
            "originals/Years/2024/2024.05.01 Coast/sub/DSC2.jpg",
            "originals/Years/2023/2023.12.31 Coast/DSC3.jpg",
            "originals/Projects/Family/a.jpg",
            "published/Years/2024/2024.05.01 Coast/a.jpg",
            "inbox/PixelMob/a.jpg",
        ]);
        assert_eq!(projects.len(), 3);
        assert_eq!(projects[0].name, "Family");
        assert_eq!(projects[1].name, "2023.12.31 Coast");
        assert_eq!(projects[2].name, "2024.05.01 Coast");
    }

    #[test]
    fn new_project_has_dated_name() {
        let date = NaiveDate::from_ymd_opt(2024, 5, 1).unwrap();
        let project = Project::new_year_project(date, "Coast").unwrap();
        assert_eq!(project.originals, "originals/Years/2024/2024.05.01 Coast");
        assert_eq!(project.name, "2024.05.01 Coast");
    }

    #[test]
    fn names_allow_glob_characters_but_not_separators() {
        assert!(Project::from_originals("originals/Projects/Trip [2024] {a}?").is_ok());
        assert!(Project::from_originals("originals/Projects/a/b").is_err());
        assert!(Project::from_originals("originals/Projects/a\nb").is_err());
    }
}
