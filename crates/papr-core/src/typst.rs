//! Embedded Typst compilation for writing projects.
//!
//! The world is deliberately filesystem-backed: Typst owns parsing and
//! incremental compilation, while Papr controls the project root, fonts,
//! package loading, diagnostics, and PDF output.

use std::{
    io::{self, Read},
    path::{Path, PathBuf},
    sync::LazyLock,
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use typst::{
    Library, LibraryExt, World, WorldExt,
    diag::{Severity, SourceDiagnostic, Warned},
    foundations::{Bytes, Datetime, Duration},
    syntax::{FileId, RootedPath, VirtualPath, VirtualRoot},
    text::{Font, FontBook},
    utils::LazyHash,
};
use typst_kit::{
    datetime::Time,
    downloader::Downloader,
    files::{FileStore, FsRoot, SystemFiles},
    fonts::{self, FontStore},
    packages::SystemPackages,
};
use typst_layout::PagedDocument;

use crate::projects::{ProjectBuildDiagnostic, ProjectDiagnosticSeverity};

/// Result of compiling a Typst project and exporting `main.pdf`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypstCompileResult {
    /// Whether a complete PDF was written.
    pub success: bool,
    /// Native Typst warnings and errors mapped to Papr's build panel model.
    pub diagnostics: Vec<ProjectBuildDiagnostic>,
    /// Human-readable output for the build panel's raw view.
    pub raw_log: Vec<String>,
}

/// A reusable embedded compiler for one Typst project.
///
/// Reusing the world preserves Typst's source cache between live builds.
pub struct TypstCompiler {
    root: PathBuf,
    world: PaprTypstWorld,
}

impl TypstCompiler {
    /// Prepare an embedded compiler rooted at `project_root`.
    ///
    /// # Errors
    ///
    /// Returns an error when the project root cannot be resolved or the
    /// embedded compiler's filesystem and package services cannot initialize.
    pub fn new(project_root: &Path) -> Result<Self> {
        let root = project_root
            .canonicalize()
            .with_context(|| format!("could not open Typst project {}", project_root.display()))?;
        let world = PaprTypstWorld::new(&root)?;
        Ok(Self { root, world })
    }

    /// Compile `main.typ` and publish a complete build result to the UI.
    /// A failed build leaves the last successful PDF in place.
    pub fn compile(&mut self) -> TypstCompileResult {
        self.world.reset();
        let Warned { output, warnings } = typst::compile::<PagedDocument>(&self.world);
        let mut native_diagnostics = warnings.into_iter().collect::<Vec<_>>();

        let pdf = match output {
            Ok(document) => match typst_pdf::pdf(&document, &typst_pdf::PdfOptions::default()) {
                Ok(pdf) => Some(pdf),
                Err(errors) => {
                    native_diagnostics.extend(errors);
                    None
                }
            },
            Err(errors) => {
                native_diagnostics.extend(errors);
                None
            }
        };

        let mut diagnostics = native_diagnostics
            .iter()
            .map(|diagnostic| map_diagnostic(&self.world, diagnostic))
            .collect::<Vec<_>>();
        let mut success = false;
        if let Some(pdf) = pdf {
            let output = self.root.join("main.pdf");
            match publish_pdf(&output, &pdf) {
                Ok(()) => success = true,
                Err(error) => diagnostics.push(ProjectBuildDiagnostic {
                    severity: ProjectDiagnosticSeverity::Error,
                    title: "Could not write PDF".into(),
                    description: format!("could not write {}: {error}", output.display()),
                    file: None,
                    line: None,
                    col: None,
                    code: None,
                    hint: None,
                }),
            }
        }

        let mut raw_log = vec!["compiling main.typ with embedded Typst".into()];
        raw_log.extend(diagnostics.iter().flat_map(format_diagnostic));
        raw_log.push(if success {
            "compiled successfully".into()
        } else {
            "compiled with errors".into()
        });
        TypstCompileResult {
            success,
            diagnostics,
            raw_log,
        }
    }
}

fn publish_pdf(output: &Path, pdf: &[u8]) -> io::Result<()> {
    let temporary = output.with_file_name(".papr-main.pdf.tmp");
    std::fs::write(&temporary, pdf)?;

    #[cfg(windows)]
    let result = std::fs::rename(&temporary, output).or_else(|error| {
        if matches!(
            error.kind(),
            io::ErrorKind::AlreadyExists | io::ErrorKind::PermissionDenied
        ) {
            std::fs::remove_file(output)?;
            std::fs::rename(&temporary, output)
        } else {
            Err(error)
        }
    });
    #[cfg(not(windows))]
    let result = std::fs::rename(&temporary, output);

    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}

struct PaprTypstWorld {
    main: FileId,
    library: LazyHash<Library>,
    fonts: LazyLock<FontStore, Box<dyn Fn() -> FontStore + Send + Sync>>,
    files: FileStore<SystemFiles>,
    now: Time,
}

impl PaprTypstWorld {
    fn new(root: &Path) -> Result<Self> {
        let virtual_main =
            VirtualPath::new("main.typ").context("main.typ is not a valid Typst virtual path")?;
        let main = RootedPath::new(VirtualRoot::Project, virtual_main).intern();

        let downloader = HttpDownloader::new()?;
        let packages = SystemPackages::new(downloader);
        let files = FileStore::new(SystemFiles::new(FsRoot::new(root.to_path_buf()), packages));

        let font_root = root.to_path_buf();
        let font_paths = std::env::var_os("TYPST_FONT_PATHS");
        let discover: Box<dyn Fn() -> FontStore + Send + Sync> =
            Box::new(move || discover_fonts(&font_root, font_paths.as_deref()));
        let fonts = LazyLock::new(discover);

        Ok(Self {
            main,
            library: LazyHash::new(Library::default()),
            fonts,
            files,
            now: Time::system(),
        })
    }

    fn reset(&mut self) {
        self.files.reset();
        self.now.reset();
    }
}

fn discover_fonts(root: &Path, configured_paths: Option<&std::ffi::OsStr>) -> FontStore {
    let mut store = FontStore::new();
    store.extend(fonts::embedded());
    store.extend(fonts::system());
    store.extend(fonts::scan(&root.join("fonts")));
    if let Some(paths) = configured_paths {
        for path in std::env::split_paths(paths) {
            store.extend(fonts::scan(&path));
        }
    }
    store
}

impl World for PaprTypstWorld {
    fn library(&self) -> &LazyHash<Library> {
        &self.library
    }

    fn book(&self) -> &LazyHash<FontBook> {
        self.fonts.book()
    }

    fn main(&self) -> FileId {
        self.main
    }

    fn source(&self, id: FileId) -> typst::diag::FileResult<typst::syntax::Source> {
        self.files.source(id)
    }

    fn file(&self, id: FileId) -> typst::diag::FileResult<Bytes> {
        self.files.file(id)
    }

    fn font(&self, index: usize) -> Option<Font> {
        self.fonts.font(index)
    }

    fn today(&self, offset: Option<Duration>) -> Option<Datetime> {
        self.now.today(offset)
    }
}

fn map_diagnostic(world: &PaprTypstWorld, diagnostic: &SourceDiagnostic) -> ProjectBuildDiagnostic {
    let severity = match diagnostic.severity {
        Severity::Error => ProjectDiagnosticSeverity::Error,
        Severity::Warning => ProjectDiagnosticSeverity::Warning,
    };
    let mut file = None;
    let mut line = None;
    let mut col = None;
    let mut code = None;

    if let Some(id) = diagnostic.span.id() {
        file = Some(match id.root() {
            VirtualRoot::Project => id.vpath().get_without_slash().to_string(),
            VirtualRoot::Package(package) => {
                format!("{package}{}", id.vpath().get_with_slash())
            }
        });
        if let (Ok(source), Some(range)) = (world.source(id), world.range(diagnostic.span))
            && let Some(line_index) = source.lines().byte_to_line(range.start)
        {
            line = Some(line_index + 1);
            col = source
                .lines()
                .byte_to_column(range.start)
                .map(|column| column + 1);
            code = source
                .text()
                .lines()
                .nth(line_index)
                .map(str::trim)
                .map(str::to_owned);
        }
    }

    let hint = (!diagnostic.hints.is_empty()).then(|| {
        diagnostic
            .hints
            .iter()
            .map(|hint| hint.v.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    });
    let description = diagnostic.message.to_string();
    ProjectBuildDiagnostic {
        severity,
        title: description.clone(),
        description,
        file,
        line,
        col,
        code,
        hint,
    }
}

fn format_diagnostic(diagnostic: &ProjectBuildDiagnostic) -> Vec<String> {
    let label = match diagnostic.severity {
        ProjectDiagnosticSeverity::Error => "error",
        ProjectDiagnosticSeverity::Warning => "warning",
    };
    let mut lines = vec![format!("{label}: {}", diagnostic.description)];
    if let Some(file) = &diagnostic.file {
        let location = match (diagnostic.line, diagnostic.col) {
            (Some(line), Some(col)) => format!("{file}:{line}:{col}"),
            (Some(line), None) => format!("{file}:{line}"),
            _ => file.clone(),
        };
        lines.push(format!("  --> {location}"));
    }
    if let Some(code) = &diagnostic.code {
        lines.push(format!("   | {code}"));
    }
    if let Some(hint) = &diagnostic.hint {
        lines.extend(hint.lines().map(|line| format!("  hint: {line}")));
    }
    lines
}

#[derive(Clone)]
struct HttpDownloader {
    client: reqwest::blocking::Client,
}

impl HttpDownloader {
    fn new() -> Result<Self> {
        let client = reqwest::blocking::Client::builder()
            .user_agent(format!("papr/{}", env!("CARGO_PKG_VERSION")))
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .context("could not initialize the Typst package downloader")?;
        Ok(Self { client })
    }
}

impl Downloader for HttpDownloader {
    fn stream(
        &self,
        _key: &dyn std::any::Any,
        url: &str,
    ) -> io::Result<(Option<usize>, Box<dyn Read>)> {
        let response = self.client.get(url).send().map_err(io::Error::other)?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(io::Error::new(io::ErrorKind::NotFound, "package not found"));
        }
        let response = response.error_for_status().map_err(io::Error::other)?;
        let length = response
            .content_length()
            .and_then(|length| length.try_into().ok());
        Ok((length, Box::new(response)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ProjectManager;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_PROJECT: AtomicU64 = AtomicU64::new(0);

    fn project(name: &str, source: &str) -> Result<PathBuf> {
        let root = std::env::temp_dir().join(format!(
            "papr-typst-{name}-{}-{}",
            std::process::id(),
            NEXT_PROJECT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root)?;
        std::fs::write(root.join("main.typ"), source)?;
        Ok(root)
    }

    #[test]
    fn compiles_typst_to_pdf_without_a_cli() -> Result<()> {
        let root = project("success", "= Embedded Typst\nThis PDF was made by Papr.")?;
        let mut compiler = TypstCompiler::new(&root)?;
        let result = compiler.compile();
        assert!(result.success, "{:?}", result.diagnostics);
        assert!(result.diagnostics.is_empty());
        assert!(std::fs::read(root.join("main.pdf"))?.starts_with(b"%PDF-"));

        std::fs::write(root.join("section.typ"), "The world was reused.")?;
        std::fs::write(
            root.join("main.typ"),
            "= Recompiled\n#include \"section.typ\"",
        )?;
        let second = compiler.compile();
        assert!(second.success, "{:?}", second.diagnostics);

        std::fs::write(root.join("section.typ"), "A dependency changed.")?;
        let third = compiler.compile();
        assert!(third.success, "{:?}", third.diagnostics);
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn exposes_structured_source_diagnostics() -> Result<()> {
        let root = project("diagnostic", "#set page(width: 10)\n")?;
        let result = TypstCompiler::new(&root)?.compile();
        assert!(!result.success);
        assert_eq!(result.diagnostics[0].file.as_deref(), Some("main.typ"));
        assert_eq!(result.diagnostics[0].line, Some(1));
        assert!(result.diagnostics[0].col.is_some());
        assert_eq!(
            result.diagnostics[0].code.as_deref(),
            Some("#set page(width: 10)")
        );
        assert!(!result.raw_log.is_empty());
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn newly_created_typst_project_compiles_end_to_end() -> Result<()> {
        let root = project("manager-root", "temporary")?;
        std::fs::remove_file(root.join("main.typ"))?;
        let manager = ProjectManager::new(root.clone())?;
        let created = manager.create("paper", "typst")?;
        let result = TypstCompiler::new(&created.path)?.compile();
        assert!(result.success, "{:?}", result.diagnostics);
        assert!(std::fs::read(created.path.join("main.pdf"))?.starts_with(b"%PDF-"));
        std::fs::remove_dir_all(root)?;
        Ok(())
    }
}
