//! Filesystem-backed LaTeX and Typst project management.
//!
//! Project metadata intentionally lives beside the configured project root so
//! it survives database rebuilds and can be used by every frontend.

#![allow(missing_docs)] // Public fields are self-describing serialized metadata.

use std::{
    fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::Arc,
    thread::JoinHandle,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::canonicalize_path;

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
    /// Create a project manager rooted at the supplied directory.
    ///
    /// # Errors
    ///
    /// Returns an error when the root cannot be created or normalized.
    pub fn new(root: PathBuf) -> Result<Self, ProjectError> {
        fs::create_dir_all(&root)?;
        // Retain a single absolute, filesystem-normalized representation for
        // managed, external, persisted, and UI-facing project paths.
        Ok(Self {
            root: canonicalize_path(root)?,
        })
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn registry_path(&self) -> PathBuf {
        self.root.join(REGISTRY_FILE)
    }

    fn read_registry(&self) -> Result<Registry, ProjectError> {
        let path = self.registry_path();
        if !path.exists() {
            return Ok(Registry::default());
        }
        let mut registry: Registry = toml::from_str(&fs::read_to_string(path)?)?;
        // Older registries may contain relative paths. Normalize at the
        // serialization boundary before any equality or containment check.
        for project in &mut registry.projects {
            if project.path.is_relative() {
                project.path = self.root.join(&project.path);
            }
            if let Ok(path) = canonicalize_path(&project.path) {
                project.path = path;
            }
        }
        Ok(registry)
    }

    fn write_registry(&self, registry: &Registry) -> Result<(), ProjectError> {
        fs::write(self.registry_path(), toml::to_string_pretty(registry)?)?;
        Ok(())
    }

    /// Discover directories in the configured root and merge external/recent
    /// projects from metadata. The registry itself is never exposed as a file.
    ///
    /// # Errors
    ///
    /// Returns an error when the project directory or registry cannot be read or
    /// the normalized registry cannot be persisted.
    pub fn list(&self) -> Result<Vec<Project>, ProjectError> {
        let mut registry = self.read_registry()?;
        let mut projects = Vec::new();
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            let opened_at = registry
                .projects
                .iter()
                .find(|p| p.path == path)
                .map_or(0, |p| p.opened_at);
            projects.push(Project {
                name,
                path,
                opened_at,
            });
        }
        registry.projects.retain(|p| p.path.is_dir());
        for project in &registry.projects {
            if !projects.iter().any(|p| p.path == project.path) {
                projects.push(project.clone());
            }
        }
        projects.sort_by(|a, b| {
            b.opened_at
                .cmp(&a.opened_at)
                .then_with(|| a.name.cmp(&b.name))
        });
        self.write_registry(&Registry {
            projects: projects.clone(),
        })?;
        Ok(projects)
    }

    /// Create a conventional, immediately-compilable LaTeX or Typst project.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid or duplicate name, or when starter files
    /// cannot be written.
    pub fn create(&self, name: &str, compiler: &str) -> Result<Project, ProjectError> {
        validate_name(name)?;
        let path = self.root.join(name);
        if path.exists() {
            return Err(ProjectError::AlreadyExists(name.into()));
        }
        fs::create_dir_all(path.join("figures"))?;
        fs::create_dir_all(path.join("sections"))?;
        if compiler == "typst" {
            fs::write(path.join("main.typ"), default_main_typ(name))?;
            fs::write(path.join("references.bib"), "% Add BibTeX entries here.\n")?;
            fs::write(path.join(".gitignore"), "*.pdf\n")?;
        } else {
            fs::write(path.join("main.tex"), default_main_tex(name))?;
            fs::write(path.join("references.bib"), "% Add BibTeX entries here.\n")?;
            fs::write(
                path.join(".gitignore"),
                "*.aux\n*.bbl\n*.blg\n*.fdb_latexmk\n*.fls\n*.log\n*.out\n*.pdf\n",
            )?;
        }
        self.open(path)
    }

    /// Add an existing project (including one outside the managed root).
    ///
    /// # Errors
    ///
    /// Returns an error when the path is not a directory, cannot be normalized,
    /// or cannot be saved to the project registry.
    pub fn open(&self, path: PathBuf) -> Result<Project, ProjectError> {
        if !path.is_dir() {
            return Err(ProjectError::NotFound(path));
        }
        let path = canonicalize_path(&path)?;
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("Untitled project")
            .to_owned();
        let project = Project {
            name,
            path,
            opened_at: now(),
        };
        let mut registry = self.read_registry()?;
        registry.projects.retain(|p| p.path != project.path);
        registry.projects.push(project.clone());
        self.write_registry(&registry)?;
        Ok(project)
    }

    /// Rename a project and update its registry entry.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid or duplicate name, or when the directory
    /// or registry cannot be updated.
    pub fn rename(&self, project: &Project, name: &str) -> Result<Project, ProjectError> {
        validate_name(name)?;
        let new_path = project.path.parent().unwrap_or(&self.root).join(name);
        if new_path != project.path && new_path.exists() {
            return Err(ProjectError::AlreadyExists(name.into()));
        }
        fs::rename(&project.path, &new_path)?;
        let renamed = Project {
            name: name.into(),
            path: canonicalize_path(new_path)?,
            opened_at: project.opened_at,
        };
        let mut registry = self.read_registry()?;
        if let Some(item) = registry
            .projects
            .iter_mut()
            .find(|p| p.path == project.path)
        {
            *item = renamed.clone();
        }
        self.write_registry(&registry)?;
        Ok(renamed)
    }

    /// Delete a project directory. Callers should show their own confirmation.
    ///
    /// # Errors
    ///
    /// Returns an error when the project no longer exists or its directory or
    /// registry entry cannot be removed.
    pub fn delete(&self, project: &Project) -> Result<(), ProjectError> {
        if !project.path.is_dir() {
            return Err(ProjectError::NotFound(project.path.clone()));
        }
        fs::remove_dir_all(&project.path)?;
        let mut registry = self.read_registry()?;
        registry.projects.retain(|p| p.path != project.path);
        self.write_registry(&registry)
    }
}

fn validate_name(name: &str) -> Result<(), ProjectError> {
    if name.trim().is_empty() || name.contains(['/', '\\']) || name == "." || name == ".." {
        Err(ProjectError::InvalidName)
    } else {
        Ok(())
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn default_main_tex(title: &str) -> String {
    format!(
        "\\documentclass[11pt]{{article}}\n\\usepackage[utf8]{{inputenc}}\n\\usepackage{{graphicx}}\n\\usepackage{{amsmath,amssymb,amsthm}}\n\\usepackage[a4paper,margin=1in]{{geometry}}\n\\usepackage{{enumerate}}\n\\usepackage{{float}}\n\n\\setlength{{\\parindent}}{{0em}}\n\n\\title{{{title}}}\n\\author{{}}\n\\date{{\\today}}\n\n\\begin{{document}}\n\\maketitle\n\n\\begin{{abstract}}\nWrite your abstract here.\n\\end{{abstract}}\n\n\\section{{Introduction}}\nStart writing.\n\n\\bibliographystyle{{plain}}\n\\bibliography{{references}}\n\\end{{document}}\n"
    )
}

fn default_main_typ(title: &str) -> String {
    format!(
        r#"#set document(title: "{title}")
#set page(paper: "a4", margin: 1in)

#align(center)[
  #text(17pt, weight: "bold")[{title}]
]

#v(2em)

= Introduction
Start writing.

#bibliography("references.bib")
"#
    )
}

/// Turn TeX's line-oriented output into diagnostics that can be displayed and navigated.
#[must_use]
pub fn parse_latex_diagnostics(log: &[String], project_root: &Path) -> Vec<ProjectBuildDiagnostic> {
    let files = tex_file_context(log);
    let locations = tex_locations(log, &files);
    let diagnostic_indexes = log
        .iter()
        .enumerate()
        .filter_map(|(index, output)| {
            latex_diagnostic(output).map(|diagnostic| (index, diagnostic))
        })
        .collect::<Vec<_>>();
    let mut diagnostics = Vec::new();
    let mut previous_location = None;

    for (position, (index, (severity, description))) in diagnostic_indexes.iter().enumerate() {
        let next_diagnostic = diagnostic_indexes
            .get(position + 1)
            .map_or(log.len(), |(index, _)| *index);
        let source_file = files[*index].clone();
        // TeX puts the useful `l.<n>` entry either immediately after an error,
        // before a package-generated message, or in the enclosing log block.
        let location = locations
            .iter()
            .filter(|location| location.index >= *index && location.index < next_diagnostic)
            .min_by_key(|location| location.index)
            .cloned()
            .or_else(|| {
                locations
                    .iter()
                    .rev()
                    .find(|location| {
                        location.index < *index && index.saturating_sub(location.index) <= 40
                    })
                    .cloned()
            })
            .or_else(|| previous_location.clone());
        let source_file = location
            .as_ref()
            .map_or(source_file, |location| location.file.clone());
        let (line, compiler_code) = location.as_ref().map_or((None, None), |location| {
            (Some(location.line), location.code.clone())
        });
        if let Some(location) = &location {
            previous_location = Some(location.clone());
        }
        let source = std::fs::read_to_string(project_root.join(&source_file)).ok();
        let code = compiler_code.or_else(|| {
            line.and_then(|number| {
                source
                    .as_deref()
                    .and_then(|contents| contents.lines().nth(number.saturating_sub(1)))
                    .map(str::to_owned)
            })
        });
        let title = diagnostic_title(*severity, description);
        let hint = match severity {
            ProjectDiagnosticSeverity::Error if title == "Emergency stop" => {
                Some("Triggered after the preceding compiler error.".into())
            }
            ProjectDiagnosticSeverity::Error if title == "Undefined control sequence" => code
                .as_ref()
                .map(|command| format!("Undefined control sequence `{command}`."))
                .or_else(|| Some(description.clone())),
            ProjectDiagnosticSeverity::Error => Some(description.clone()),
            ProjectDiagnosticSeverity::Warning => None,
        };
        diagnostics.push(ProjectBuildDiagnostic {
            severity: *severity,
            title,
            description: description.clone(),
            file: Some(source_file),
            line,
            col: None,
            code,
            hint,
        });
    }
    diagnostics
}

/// Lifecycle signal inferred from one line emitted by `latexmk`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LatexBuildSignal {
    /// A compilation pass began.
    Started,
    /// The build completed successfully.
    Succeeded,
    /// The build completed with errors.
    Failed,
}

/// Structured events emitted by a continuously running `latexmk` process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LatexBuildEvent {
    /// Raw compiler output.
    LogLine(String),
    /// A compilation lifecycle transition.
    Signal(LatexBuildSignal),
}

/// Owns the reusable `latexmk` process lifecycle and output readers.
pub struct LatexBuildProcess {
    child: Child,
    readers: Vec<JoinHandle<()>>,
    stopped: bool,
}

impl LatexBuildProcess {
    /// Start continuous PDF compilation and forward structured events.
    ///
    /// # Errors
    ///
    /// Returns an error when the `latexmk` process cannot be spawned.
    pub fn start(
        project_root: &Path,
        on_event: impl Fn(LatexBuildEvent) + Send + Sync + 'static,
    ) -> Result<Self, std::io::Error> {
        let mut command = Command::new("latexmk");
        command
            .args(["-pdf", "-pvc", "-view=none", "main.tex"])
            .current_dir(project_root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = command.spawn()?;
        let on_event = Arc::new(on_event);
        let mut readers = Vec::with_capacity(2);
        if let Some(stdout) = child.stdout.take() {
            readers.push(spawn_latexmk_reader(stdout, on_event.clone()));
        }
        if let Some(stderr) = child.stderr.take() {
            readers.push(spawn_latexmk_reader(stderr, on_event));
        }
        Ok(Self {
            child,
            readers,
            stopped: false,
        })
    }

    /// Stop `latexmk` and all active TeX subprocesses, then join output readers.
    pub fn stop(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        #[cfg(unix)]
        {
            let process_group = format!("-{}", self.child.id());
            let _ = Command::new("kill")
                .args(["-KILL", "--", &process_group])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        for reader in self.readers.drain(..) {
            let _ = reader.join();
        }
    }
}

impl Drop for LatexBuildProcess {
    fn drop(&mut self) {
        self.stop();
    }
}

fn spawn_latexmk_reader(
    reader: impl std::io::Read + Send + 'static,
    on_event: Arc<impl Fn(LatexBuildEvent) + Send + Sync + 'static>,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        for line in BufReader::new(reader).lines().map_while(Result::ok) {
            on_event(LatexBuildEvent::LogLine(line.clone()));
            if let Some(signal) = classify_latexmk_line(&line) {
                on_event(LatexBuildEvent::Signal(signal));
            }
        }
    })
}

/// Classify `latexmk` output without coupling process execution to a frontend.
#[must_use]
pub fn classify_latexmk_line(line: &str) -> Option<LatexBuildSignal> {
    let normalized = line.to_ascii_lowercase();
    if normalized.contains("applying rule 'pdflatex'") || normalized.contains("compiling ...") {
        Some(LatexBuildSignal::Started)
    } else if (normalized.contains("all targets") && normalized.contains("up-to-date"))
        || normalized.contains("compiled successfully")
    {
        Some(LatexBuildSignal::Succeeded)
    } else if normalized.contains("errors, so i did not")
        || normalized.contains("compiled with errors")
    {
        Some(LatexBuildSignal::Failed)
    } else {
        None
    }
}

#[derive(Clone)]
struct TexLocation {
    index: usize,
    file: String,
    line: usize,
    code: Option<String>,
}

fn latex_diagnostic(output: &str) -> Option<(ProjectDiagnosticSeverity, String)> {
    let text = output.trim_start();
    if let Some(error) = text.strip_prefix('!') {
        let error = error.trim_start();
        return (!error.is_empty() && !error.contains("Fatal error occurred"))
            .then(|| (ProjectDiagnosticSeverity::Error, error.to_owned()));
    }
    if let Some((_, error)) = text.rsplit_once("LaTeX Error:") {
        return Some((ProjectDiagnosticSeverity::Error, error.trim().to_owned()));
    }
    if text.starts_with("Package ") && text.contains(" Error:") {
        return Some((ProjectDiagnosticSeverity::Error, text.to_owned()));
    }
    if file_line_number(text).is_some() && text.to_ascii_lowercase().contains("error") {
        return Some((ProjectDiagnosticSeverity::Error, text.to_owned()));
    }
    (text.starts_with("LaTeX Warning:")
        || (text.starts_with("Package ") && text.contains(" Warning:"))
        || text.starts_with("Overfull \\hbox")
        || text.starts_with("Underfull \\hbox"))
    .then(|| (ProjectDiagnosticSeverity::Warning, text.to_owned()))
}

fn tex_file_context(log: &[String]) -> Vec<String> {
    let mut stack = vec!["main.tex".to_owned()];
    log.iter()
        .map(|output| {
            let references = tex_file_references(output);
            for file in &references {
                if stack.last() != Some(file) {
                    stack.push(file.clone());
                }
            }
            let current = references
                .last()
                .cloned()
                .unwrap_or_else(|| stack.last().cloned().unwrap_or_else(|| "main.tex".into()));
            let closes = output.chars().filter(|character| *character == ')').count();
            for _ in 0..closes {
                if stack.len() > 1 {
                    stack.pop();
                }
            }
            current
        })
        .collect()
}

fn tex_locations(log: &[String], files: &[String]) -> Vec<TexLocation> {
    log.iter()
        .enumerate()
        .filter_map(|(index, output)| {
            let (line, code) = compiler_location_in_line(output)?;
            let file = tex_file_references(output)
                .last()
                .cloned()
                .unwrap_or_else(|| files[index].clone());
            Some(TexLocation {
                index,
                file,
                line,
                code,
            })
        })
        .collect()
}

fn tex_file_references(output: &str) -> Vec<String> {
    output
        .split(|character: char| {
            character.is_whitespace() || matches!(character, '(' | ')' | '[' | ']' | '"' | '\'')
        })
        .filter_map(|candidate| {
            let candidate = candidate.trim_start_matches("./");
            [".tex", ".sty", ".cls", ".ltx", ".bib"]
                .iter()
                .find_map(|extension| {
                    candidate
                        .find(extension)
                        .map(|index| candidate[..index + extension.len()].to_owned())
                })
        })
        .collect()
}

fn compiler_location_in_line(output: &str) -> Option<(usize, Option<String>)> {
    let text = output.trim();
    if let Some(offset) = text.find("l.") {
        let rest = text[offset + 2..].trim_start();
        let digits = rest
            .chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>();
        if let Ok(line) = digits.parse() {
            let code = rest[digits.len()..].trim();
            return Some((line, (!code.is_empty()).then(|| code.to_owned())));
        }
    }

    let lower = text.to_ascii_lowercase();
    for marker in [
        "on input line ",
        "on line ",
        "at line ",
        "at lines ",
        "line ",
    ] {
        if let Some(offset) = lower.find(marker)
            && let Some(line) = parse_line_number(&lower[offset + marker.len()..])
        {
            return Some((line, None));
        }
    }

    for extension in [".tex", ".sty", ".cls", ".ltx", ".bib"] {
        if let Some(offset) = lower.find(extension) {
            let rest = lower[offset + extension.len()..].strip_prefix(':')?;
            if let Some(line) = parse_line_number(rest) {
                return Some((line, None));
            }
        }
    }
    None
}

fn file_line_number(output: &str) -> Option<usize> {
    let lower = output.to_ascii_lowercase();
    for extension in [".tex", ".sty", ".cls", ".ltx", ".bib"] {
        if let Some(offset) = lower.find(extension) {
            let rest = lower[offset + extension.len()..].strip_prefix(':')?;
            if let Some(line) = parse_line_number(rest) {
                return Some(line);
            }
        }
    }
    None
}

fn parse_line_number(text: &str) -> Option<usize> {
    let digits = text
        .trim_start_matches(|character: char| !character.is_ascii_digit())
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>();
    digits.parse().ok()
}

fn diagnostic_title(severity: ProjectDiagnosticSeverity, description: &str) -> String {
    for title in [
        "Undefined control sequence",
        "Missing $ inserted",
        "Missing } inserted",
        "Emergency stop",
    ] {
        if description.starts_with(title) {
            return title.to_owned();
        }
    }
    if description.starts_with("Overfull \\hbox") {
        return "Overfull \\hbox".into();
    }
    if description.starts_with("Underfull \\hbox") {
        return "Underfull \\hbox".into();
    }
    match severity {
        ProjectDiagnosticSeverity::Error => "LaTeX Error".into(),
        ProjectDiagnosticSeverity::Warning => "LaTeX Warning".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn latex_diagnostic_parser_enriches_errors_and_warnings() {
        let log = vec![
            "! Undefined control sequence.".into(),
            "l.17 \\uias".into(),
            "LaTeX Warning: Citation `example' undefined on input line 24.".into(),
            "Latexmk: Errors, so I did not complete making targets".into(),
        ];
        let diagnostics = parse_latex_diagnostics(&log, std::path::Path::new("."));

        assert_eq!(diagnostics.len(), 2);
        assert_eq!(diagnostics[0].title, "Undefined control sequence");
        assert_eq!(diagnostics[0].line, Some(17));
        assert_eq!(diagnostics[0].code.as_deref(), Some("\\uias"));
        assert_eq!(diagnostics[1].line, Some(24));
    }

    #[test]
    fn latex_diagnostic_parser_carries_locations_across_related_errors_and_formats() {
        let log = vec![
            "! LaTeX Error: Environment equation undefined.".into(),
            "See the LaTeX manual or LaTeX Companion for explanation.".into(),
            "l.23 \\begin{equation}".into(),
            "! Emergency stop.".into(),
            "<*> main.tex".into(),
            "./main.tex:42: LaTeX Error: Missing \\begin{document}.".into(),
            "Package hyperref Warning: Token not allowed in a PDF string on line 51.".into(),
        ];
        let diagnostics = parse_latex_diagnostics(&log, std::path::Path::new("."));

        assert_eq!(diagnostics.len(), 4);
        assert_eq!(diagnostics[0].line, Some(23));
        assert_eq!(diagnostics[0].code.as_deref(), Some("\\begin{equation}"));
        assert_eq!(diagnostics[1].title, "Emergency stop");
        assert_eq!(diagnostics[1].line, Some(23));
        assert_eq!(diagnostics[2].line, Some(42));
        assert_eq!(diagnostics[3].line, Some(51));
    }

    #[test]
    fn latex_diagnostic_parser_tracks_nested_files_and_engine_style_locations() {
        let log = vec![
            "(./chapters/intro.tex".into(),
            "! Package amsmath Error: Bad math environment delimiter.".into(),
            "l. 71 \\begin{equation}".into(),
            ")".into(),
            "./main.tex:14: error: undefined control sequence".into(),
        ];
        let diagnostics = parse_latex_diagnostics(&log, std::path::Path::new("."));

        assert_eq!(diagnostics.len(), 2);
        assert_eq!(diagnostics[0].file.as_deref(), Some("chapters/intro.tex"));
        assert_eq!(diagnostics[0].line, Some(71));
        assert_eq!(diagnostics[1].file.as_deref(), Some("main.tex"));
        assert_eq!(diagnostics[1].line, Some(14));
    }

    #[test]
    fn creates_discovers_and_persists_external_projects() -> Result<(), Box<dyn std::error::Error>>
    {
        let root = std::env::temp_dir().join(format!("papr-projects-{}", now()));
        let manager = ProjectManager::new(root.clone())?;
        let created = manager.create("paper", "latex")?;
        assert!(created.path.join("main.tex").exists());
        let main = fs::read_to_string(created.path.join("main.tex"))?;
        assert!(main.contains("\\title{paper}"));
        assert!(main.contains("\\usepackage[a4paper,margin=1in]{geometry}"));
        assert!(main.contains("\\usepackage{enumerate}"));
        assert!(main.contains("\\usepackage{float}"));
        assert!(main.contains("\\usepackage{amsmath,amssymb,amsthm}"));
        assert!(!main.contains("\\usepackage{parskip}"));
        assert!(main.contains("\\setlength{\\parindent}{0em}"));
        assert!(!main.contains("\\setlength{\\parskip}"));
        assert_eq!(manager.list()?.len(), 1);
        let parent = root
            .parent()
            .ok_or_else(|| std::io::Error::other("temporary project root has no parent"))?;
        let external = parent.join(format!("papr-external-{}", now()));
        fs::create_dir_all(&external)?;
        manager.open(external.clone())?;
        assert_eq!(manager.list()?.len(), 2);
        fs::remove_dir_all(root)?;
        fs::remove_dir_all(external)?;
        Ok(())
    }

    #[test]
    fn normalizes_root_and_legacy_registry_paths() -> Result<(), Box<dyn std::error::Error>> {
        let root = std::env::temp_dir().join(format!(
            "papr-project-paths-{}",
            SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
        ));
        let manager = ProjectManager::new(root.clone())?;
        let created = manager.create("paper", "latex")?;
        assert!(created.path.is_absolute());
        assert_eq!(created.path, manager.root().join("paper"));
        #[cfg(target_os = "windows")]
        assert!(!created.path.to_string_lossy().starts_with(r"\\?\"));

        fs::write(
            manager.registry_path(),
            "[[projects]]\nname = \"paper\"\npath = \"paper\"\nopened_at = 1\n",
        )?;
        let projects = manager.list()?;
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].path, created.path);
        assert!(
            fs::read_to_string(manager.registry_path())?
                .contains(&created.path.display().to_string())
        );
        fs::remove_dir_all(root)?;
        Ok(())
    }
}

/// Severity assigned to a parsed LaTeX compiler diagnostic.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ProjectDiagnosticSeverity {
    /// Compilation cannot complete successfully.
    Error,
    /// Compilation completed but reported a condition needing attention.
    Warning,
}

/// A compiler diagnostic enriched with source location and context when available.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectBuildDiagnostic {
    /// Error or warning severity.
    pub severity: ProjectDiagnosticSeverity,
    /// Short, scannable diagnostic category.
    pub title: String,
    /// Compiler-provided description.
    pub description: String,
    /// Project-relative source path when known.
    pub file: Option<String>,
    /// One-based source line when known.
    pub line: Option<usize>,
    /// One-based source column when known.
    pub col: Option<usize>,
    /// Source text at the reported location when available.
    pub code: Option<String>,
    /// Additional compiler context or remediation hint.
    pub hint: Option<String>,
}
