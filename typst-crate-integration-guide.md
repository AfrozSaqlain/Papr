# Typst Crate Integration: Autonomous Agent Implementation Guide

**Project:** Papr (Keyboard-First Terminal Workspace for Academic Papers)  
**Date:** August 7, 2026  
**Document Type:** Step-by-Step Execution Playbook & Technical Specification  
**Target Execution Environment:** Cargo Rust Workspace (`crates/papr-core`, `crates/papr`)  

---

## 🎯 Purpose & Scope

This guide provides a complete, deterministic, step-by-step blueprint for an autonomous AI coding agent or software developer to embed the official [`typst`](https://crates.io/crates/typst) library suite into **Papr** as a native Cargo crate dependency.

Following this playbook will:
1. Eliminate the requirement for users to install an external `typst` binary executable.
2. Enable in-memory sub-millisecond document compilation and native structured diagnostics.
3. Preserve existing `latexmk` subprocess compilation for LaTeX projects.
4. Maintain 100% compatibility with `cargo-dist`, Crates.io, Debian/RPM Linux packages, Arch AUR, Homebrew, and WinGet.

---

## 📋 Pre-Flight Verification Checklist

Before executing any edits, verify workspace status by running:
```bash
cargo check --workspace
cargo test --workspace
```
*Requirement*: All checks and tests must pass cleanly before starting Phase 1.

---

## 🧩 Phase 1: Workspace Cargo Dependency Configuration

### Step 1.1: Update `crates/papr-core/Cargo.toml`
Add the Typst library stack to `crates/papr-core/Cargo.toml` dependencies:

```toml
[dependencies]
# Existing dependencies...
anyhow.workspace = true
chrono.workspace = true
directories.workspace = true
dunce.workspace = true
futures-util.workspace = true
iana-time-zone.workspace = true
notify.workspace = true
quick-xml.workspace = true
reqwest.workspace = true
rusqlite.workspace = true
serde.workspace = true
serde_json.workspace = true
sha2.workspace = true
thiserror.workspace = true
tokio.workspace = true
toml.workspace = true
url.workspace = true
walkdir.workspace = true

# Native Typst Compiler Stack
typst = "0.12"
typst-pdf = "0.12"
typst-render = "0.12"
typst-kit = "0.12"
fontdb = "0.16"
```

### Step 1.2: Update Root `Cargo.toml` (if centralized in workspace)
If dependencies are declared under `[workspace.dependencies]` in the root `Cargo.toml`, add:

```toml
[workspace.dependencies]
# Add:
typst = "0.12"
typst-pdf = "0.12"
typst-render = "0.12"
typst-kit = "0.12"
fontdb = "0.16"
```

---

## 🛠️ Phase 2: Implement `PaprTypstWorld` in `crates/papr-core`

Create a new file: [`crates/papr-core/src/typst_world.rs`](file:///home/qubit/Documents/Papr_Code/crates/papr-core/src/typst_world.rs).

This module implements the `typst::World` trait to connect the Typst compiler engine with Papr's filesystem, font loading, and time context.

### Code Template: `crates/papr-core/src/typst_world.rs`

```rust
//! Implementation of `typst::World` for Papr's in-memory compilation pipeline.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use chrono::{DateTime, Datelike, Utc};
use fontdb::Database as FontDb;
use typst::{
    diag::{FileError, FileResult},
    foundations::{Bytes, Datetime},
    syntax::{FileId, Source, VirtualPath},
    text::{Font, FontBook},
    World,
};

/// A thread-safe, filesystem-backed `World` implementation for Papr.
pub struct PaprTypstWorld {
    /// Root directory of the academic paper project.
    project_root: PathBuf,
    /// Main source file ID (`main.typ`).
    main_id: FileId,
    /// System and embedded font registry metadata.
    font_book: FontBook,
    /// Loaded font binaries stored in memory.
    fonts: Vec<Font>,
    /// Cache of loaded source files.
    sources: Mutex<HashMap<FileId, Source>>,
    /// Cache of raw file bytes (images, pdfs, data).
    file_bytes: Mutex<HashMap<FileId, Bytes>>,
    /// Fixed build timestamp.
    now: DateTime<Utc>,
}

impl PaprTypstWorld {
    /// Create a new `PaprTypstWorld` initialized for a specific project directory.
    pub fn new(project_root: &Path) -> Result<Self, String> {
        let root = project_root
            .canonicalize()
            .map_err(|e| format!("Invalid project root path: {e}"))?;
        
        let main_path = root.join("main.typ");
        let vpath = VirtualPath::new("main.typ");
        let main_id = FileId::new(None, vpath);

        let main_content = std::fs::read_to_string(&main_path)
            .map_err(|e| format!("Failed to read main.typ: {e}"))?;
        let main_source = Source::new(main_id, main_content);

        let mut sources = HashMap::new();
        sources.insert(main_id, main_source);

        // Discover system fonts using fontdb
        let mut font_db = FontDb::new();
        font_db.load_system_fonts();

        let mut font_book = FontBook::new();
        let mut fonts = Vec::new();

        for face in font_db.faces() {
            if let fontdb::Source::File(ref path) = face.source {
                if let Ok(data) = std::fs::read(path) {
                    let bytes = Bytes::new(data);
                    for font in Font::iter(bytes) {
                        font_book.push(font.info().clone());
                        fonts.push(font);
                    }
                }
            }
        }

        Ok(Self {
            project_root: root,
            main_id,
            font_book,
            fonts,
            sources: Mutex::new(sources),
            file_bytes: Mutex::new(HashMap::new()),
            now: Utc::now(),
        })
    }
}

impl World for PaprTypstWorld {
    fn main(&self) -> Source {
        let sources = self.sources.lock().expect("lock sources");
        sources.get(&self.main_id).expect("main source exists").clone()
    }

    fn source(&self, id: FileId) -> FileResult<Source> {
        let mut sources = self.sources.lock().expect("lock sources");
        if let Some(source) = sources.get(&id) {
            return Ok(source.clone());
        }

        let relative_path = id.vpath().as_rootless_path();
        let real_path = self.project_root.join(relative_path);

        let content = std::fs::read_to_string(&real_path)
            .map_err(|_| FileError::NotFound(real_path))?;
        
        let source = Source::new(id, content);
        sources.insert(id, source.clone());
        Ok(source)
    }

    fn book(&self) -> &FontBook {
        &self.font_book
    }

    fn font(&self, index: usize) -> Option<Font> {
        self.fonts.get(index).cloned()
    }

    fn file(&self, id: FileId) -> FileResult<Bytes> {
        let mut bytes_map = self.file_bytes.lock().expect("lock file bytes");
        if let Some(bytes) = bytes_map.get(&id) {
            return Ok(bytes.clone());
        }

        let relative_path = id.vpath().as_rootless_path();
        let real_path = self.project_root.join(relative_path);

        let data = std::fs::read(&real_path)
            .map_err(|_| FileError::NotFound(real_path))?;
        
        let bytes = Bytes::new(data);
        bytes_map.insert(id, bytes.clone());
        Ok(bytes)
    }

    fn today(&self, offset: Option<i64>) -> Option<Datetime> {
        let naive = self.now.naive_utc();
        Datetime::from_ymd(naive.year(), naive.month() as u8, naive.day() as u8)
    }
}
```

---

## ⚡ Phase 3: High-Level Compiler API in `crates/papr-core`

Create [`crates/papr-core/src/typst_compiler.rs`](file:///home/qubit/Documents/Papr_Code/crates/papr-core/src/typst_compiler.rs) to provide a clean execution interface:

```rust
//! High-level in-memory Typst compiler interface.

use std::path::Path;
use crate::{typst_world::PaprTypstWorld, ProjectBuildDiagnostic, ProjectDiagnosticSeverity};
use typst::diag::Severity;

/// Output result of an in-memory Typst compilation.
pub struct TypstCompileResult {
    /// Compiled PDF bytes if compilation succeeded.
    pub pdf_bytes: Option<Vec<u8>>,
    /// Structured project diagnostics (warnings and errors).
    pub diagnostics: Vec<ProjectBuildDiagnostic>,
    /// Whether compilation produced a valid output document.
    pub success: bool,
}

/// Compile a Typst project directory in-memory and write output to `main.pdf`.
pub fn compile_typst_project(project_root: &Path) -> TypstCompileResult {
    let world = match PaprTypstWorld::new(project_root) {
        Ok(w) => w,
        Err(err_msg) => {
            return TypstCompileResult {
                pdf_bytes: None,
                diagnostics: vec![ProjectBuildDiagnostic {
                    severity: ProjectDiagnosticSeverity::Error,
                    title: "Typst Environment Error".to_string(),
                    description: err_msg,
                    file: Some("main.typ".to_string()),
                    line: None,
                    col: None,
                    code: None,
                    hint: None,
                }],
                success: false,
            };
        }
    };

    let warned_doc = typst::compile(&world);
    let mut diagnostics = Vec::new();

    // Map Typst warnings and errors directly to ProjectBuildDiagnostic
    for diag in warned_doc.warnings.iter().chain(
        warned_doc.output.as_ref().err().into_iter().flat_map(|e| e.iter())
    ) {
        let severity = match diag.severity {
            Severity::Error => ProjectDiagnosticSeverity::Error,
            Severity::Warning => ProjectDiagnosticSeverity::Warning,
        };

        diagnostics.push(ProjectBuildDiagnostic {
            severity,
            title: diag.message.to_string(),
            description: diag.message.to_string(),
            file: Some("main.typ".to_string()),
            line: None, // Line numbers extracted from diag.span if available
            col: None,
            code: None,
            hint: diag.hints.first().map(|h| h.to_string()),
        });
    }

    match warned_doc.output {
        Ok(document) => {
            let pdf_bytes = typst_pdf::pdf(&document, &typst_pdf::PdfOptions::default());
            let pdf_path = project_root.join("main.pdf");
            let _ = std::fs::write(&pdf_path, &pdf_bytes);

            TypstCompileResult {
                pdf_bytes: Some(pdf_bytes),
                diagnostics,
                success: true,
            }
        }
        Err(_) => TypstCompileResult {
            pdf_bytes: None,
            diagnostics,
            success: false,
        },
    }
}
```

### Expose in `crates/papr-core/src/lib.rs`
Add to `crates/papr-core/src/lib.rs`:
```rust
pub mod typst_world;
pub mod typst_compiler;

pub use typst_compiler::{compile_typst_project, TypstCompileResult};
```

---

## 🖥️ Phase 4: Integrate into `crates/papr/src/main.rs`

In `crates/papr/src/main.rs`, update the project build watcher task (`ProjectBuildWatcher`):

### Refactor Process Spawning Logic
Replace `ProcessCommand::new("typst")` subprocess spawning with direct in-memory calls to `papr_core::compile_typst_project`:

```rust
// Inside ProjectBuildWatcher loop or spawn logic:
if is_typst {
    // Native In-Memory Compilation Path (Zero Subprocess Required)
    let compile_result = papr_core::compile_typst_project(&project.path);
    
    // Update TUI diagnostics directly
    app.project_build_diagnostics = compile_result.diagnostics;
    
    if compile_result.success {
        let _ = watch_sender.send(ProjectBuildEvent::PdfChanged);
    }
} else {
    // Retain existing latexmk subprocess compilation path for TeX projects
    let mut command = ProcessCommand::new("latexmk");
    command.args(["-pdf", "-pvc", "-view=none", "main.tex"]);
    // ... existing latexmk process handling ...
}
```

---

## 🧪 Phase 5: Test Verification & Validation

Execute the full verification protocol:

```bash
# 1. Verify compilation and lint clean state
cargo check --workspace
cargo clippy --workspace -- -D warnings

# 2. Run unit and integration tests
cargo test --workspace

# 3. Test Typst sample document build
cargo run -p papr-tui
```

---

## 📦 Phase 6: Release Pipeline & Package Integrity

Run a dry-run release verification using `cargo-dist`:

```bash
cargo dist plan
```

*Verification Goals*:
- `papr-installer.sh` and `papr-installer.ps1` continue to generate without error.
- `cargo deb -p papr-tui` builds the `.deb` package successfully.
- `cargo generate-rpm` builds the `.rpm` package successfully.
- Crates.io publish dry-run succeeds (`cargo publish --dry-run -p papr-tui`).

---

## 🛡️ Safety & Fallback Principles for AI Agents

1. **Do Not Touch LaTeX Logic**: Leave all `latexmk` diagnostic parsers (`parse_latex_diagnostics`) and process runners untouched.
2. **Preserve Public API Signatures**: Ensure `parse_typst_diagnostics` remains in `crates/papr-core/src/projects.rs` as a fallback or for backwards compatibility.
3. **No Unwanted Subprocess Invocation**: When `is_typst` is `true`, no `ProcessCommand::new("typst")` should be spawned.
4. **Idempotency**: All changes must be fully reproducible across Linux, macOS, and Windows build targets.

---
*Playbook generated for the Papr codebase.*
