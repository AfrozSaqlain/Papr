//! Filesystem-backed LaTeX project management.
//!
//! Project metadata intentionally lives beside the configured project root so
//! it survives database rebuilds and can be used by every frontend.

#![allow(missing_docs)] // Public fields are self-describing serialized metadata.

use std::{fs, path::{Path, PathBuf}, time::{SystemTime, UNIX_EPOCH}};

use serde::{Deserialize, Serialize};
use thiserror::Error;

const REGISTRY_FILE: &str = ".papr-projects.toml";

#[derive(Debug, Error)]
pub enum ProjectError {
    #[error("project name is empty or contains a path separator")]
    InvalidName,
    #[error("a project named {0:?} already exists")]
    AlreadyExists(String),
    #[error("project directory does not exist: {0}")]
    NotFound(PathBuf),
    #[error("project I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid project registry: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("could not write project registry: {0}")]
    Serialize(#[from] toml::ser::Error),
}

/// A project known to Papr.  `path` may be outside the configured root.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Project {
    pub name: String,
    pub path: PathBuf,
    pub opened_at: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Registry {
    #[serde(default)]
    projects: Vec<Project>,
}

/// Owns project discovery, starter creation, and persistent recent-project
/// metadata. It contains no UI or process-management policy.
#[derive(Debug, Clone)]
pub struct ProjectManager {
    root: PathBuf,
}

impl ProjectManager {
    pub fn new(root: PathBuf) -> Result<Self, ProjectError> {
        fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    #[must_use]
    pub fn root(&self) -> &Path { &self.root }

    fn registry_path(&self) -> PathBuf { self.root.join(REGISTRY_FILE) }

    fn read_registry(&self) -> Result<Registry, ProjectError> {
        let path = self.registry_path();
        if !path.exists() { return Ok(Registry::default()); }
        Ok(toml::from_str(&fs::read_to_string(path)?)?)
    }

    fn write_registry(&self, registry: &Registry) -> Result<(), ProjectError> {
        fs::write(self.registry_path(), toml::to_string_pretty(registry)?)?;
        Ok(())
    }

    /// Discover directories in the configured root and merge external/recent
    /// projects from metadata. The registry itself is never exposed as a file.
    pub fn list(&self) -> Result<Vec<Project>, ProjectError> {
        let mut registry = self.read_registry()?;
        let mut projects = Vec::new();
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() { continue; }
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            let opened_at = registry.projects.iter().find(|p| p.path == path).map_or(0, |p| p.opened_at);
            projects.push(Project { name, path, opened_at });
        }
        registry.projects.retain(|p| p.path.is_dir());
        for project in &registry.projects {
            if !projects.iter().any(|p| p.path == project.path) { projects.push(project.clone()); }
        }
        projects.sort_by(|a, b| b.opened_at.cmp(&a.opened_at).then_with(|| a.name.cmp(&b.name)));
        self.write_registry(&Registry { projects: projects.clone() })?;
        Ok(projects)
    }

    /// Create a conventional, immediately-compilable LaTeX project.
    pub fn create(&self, name: &str) -> Result<Project, ProjectError> {
        validate_name(name)?;
        let path = self.root.join(name);
        if path.exists() { return Err(ProjectError::AlreadyExists(name.into())); }
        fs::create_dir_all(path.join("figures"))?;
        fs::create_dir_all(path.join("sections"))?;
        fs::write(path.join("main.tex"), default_main_tex(name))?;
        fs::write(path.join("references.bib"), "% Add BibTeX entries here.\n")?;
        fs::write(path.join(".gitignore"), "*.aux\n*.bbl\n*.blg\n*.fdb_latexmk\n*.fls\n*.log\n*.out\n*.pdf\n")?;
        self.open(path)
    }

    /// Add an existing project (including one outside the managed root).
    pub fn open(&self, path: PathBuf) -> Result<Project, ProjectError> {
        if !path.is_dir() { return Err(ProjectError::NotFound(path)); }
        let path = fs::canonicalize(&path).unwrap_or(path);
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("Untitled project").to_owned();
        let project = Project { name, path, opened_at: now() };
        let mut registry = self.read_registry()?;
        registry.projects.retain(|p| p.path != project.path);
        registry.projects.push(project.clone());
        self.write_registry(&registry)?;
        Ok(project)
    }

    pub fn rename(&self, project: &Project, name: &str) -> Result<Project, ProjectError> {
        validate_name(name)?;
        let new_path = project.path.parent().unwrap_or(&self.root).join(name);
        if new_path != project.path && new_path.exists() { return Err(ProjectError::AlreadyExists(name.into())); }
        fs::rename(&project.path, &new_path)?;
        let renamed = Project { name: name.into(), path: new_path, opened_at: project.opened_at };
        let mut registry = self.read_registry()?;
        if let Some(item) = registry.projects.iter_mut().find(|p| p.path == project.path) { *item = renamed.clone(); }
        self.write_registry(&registry)?;
        Ok(renamed)
    }

    /// Delete a project directory. Callers should show their own confirmation.
    pub fn delete(&self, project: &Project) -> Result<(), ProjectError> {
        if !project.path.is_dir() { return Err(ProjectError::NotFound(project.path.clone())); }
        fs::remove_dir_all(&project.path)?;
        let mut registry = self.read_registry()?;
        registry.projects.retain(|p| p.path != project.path);
        self.write_registry(&registry)
    }
}

fn validate_name(name: &str) -> Result<(), ProjectError> {
    if name.trim().is_empty() || name.contains(['/', '\\']) || name == "." || name == ".." { Err(ProjectError::InvalidName) } else { Ok(()) }
}

fn now() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() }

fn default_main_tex(title: &str) -> String {
    format!("\\documentclass[11pt]{{article}}\n\\usepackage[utf8]{{inputenc}}\n\\usepackage{{graphicx}}\n\\usepackage{{amsmath}}\n\\title{{{title}}}\n\\author{{}}\n\\date{{\\today}}\n\n\\begin{{document}}\n\\maketitle\n\n\\begin{{abstract}}\nWrite your abstract here.\n\\end{{abstract}}\n\n\\section{{Introduction}}\nStart writing.\n\n\\bibliographystyle{{plain}}\n\\bibliography{{references}}\n\\end{{document}}\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn creates_discovers_and_persists_external_projects() -> Result<(), Box<dyn std::error::Error>> {
        let root = std::env::temp_dir().join(format!("papr-projects-{}", now()));
        let manager = ProjectManager::new(root.clone())?;
        let created = manager.create("paper")?;
        assert!(created.path.join("main.tex").exists());
        assert!(fs::read_to_string(created.path.join("main.tex"))?.contains("\\title{paper}"));
        assert_eq!(manager.list()?.len(), 1);
        let external = root.parent().unwrap().join(format!("papr-external-{}", now()));
        fs::create_dir_all(&external)?;
        manager.open(external.clone())?;
        assert_eq!(manager.list()?.len(), 2);
        fs::remove_dir_all(root)?;
        fs::remove_dir_all(external)?;
        Ok(())
    }
}
