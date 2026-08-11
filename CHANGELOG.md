# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.1] - 2026-08-11

### Added
- **Typst Integration**: Added Typst compiler support as default and in-built functionality.
- **Enhanced Citation Workflow**:
  - Imprt citations directly from online & local library into a project's BibTeX file using `Ctrl + F`.
  - Citation autocomplete now displays paper titles instead of raw BibTeX keys.
  - Citation importer modal displays author names while preserving author ordering.
- **Dashboard Paper Tracking**: Dashboard tracks displayed papers over a 7-day window to guarantee unique paper recommendations.
- **Localized Timestamps**: Rendered user-visible timestamps in the local system timezone instead of UTC.

### Changed
- **Architecture Refactoring**: Cleanly separated `papr-core` backend logic from TUI presentation layers.
- **Vim Motions & Editor**: Expanded Vim motion navigation and improved editor key bindings.

### Fixed
- **PDF Navigation**: Smoother PDF document scrolling performance.
- **Rendering & Graphics**: Resolved UI modal rendering glitches, PDF viewport alignment, and Kitty terminal graphics protocol compatibility.
- **Feed Caching**: Prevented empty feed responses from being saved as valid cache entries.

### Performance
- Optimized workspace memory utilization and resource footprint.

## [0.1.0] - 2026-02-01

### Added
- Initial public release of Papr.
