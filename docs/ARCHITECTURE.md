# Architecture

papr is a Cargo workspace with two ownership boundaries:

- papr-core owns domain models, SQLite migrations and queries, API clients,
  configuration, library indexing, downloads, themes, and the plugin protocol.
- papr owns terminal lifecycle, keyboard mapping, asynchronous orchestration,
  external viewer launching, and ratatui rendering.

Writing projects share one build-event contract. LaTeX projects keep a
persistent `latexmk` subprocess, while Typst projects compile in a background
thread through `papr-core`'s embedded Typst world. The world provides
project-rooted files, lazy system and bundled fonts, cached Typst Universe packages,
native source diagnostics, and PDF export. Filesystem events are debounced and
the world is reused so live builds retain Typst's incremental source cache.

The UI mutates an explicit App state machine through semantic actions.
Network requests, hashing, filesystem events, and downloads communicate with
the event loop through Tokio channels. Rendering never performs I/O.

## Persistence

SQLite is the source of truth. Migrations are append-only, versioned SQL files
applied transactionally and recorded in schema_migrations. Foreign keys are
enabled for every connection. WAL mode supports responsive reads.

## Extension boundary

Plugins are child processes using a versioned JSON protocol. They cannot share
memory with papr or rely on Rust's unstable ABI. Manifests declare capabilities,
configuration provides an explicit execution allowlist, and every invocation
has a deadline and response-size bound.

## Future AI features

Future optional AI functionality belongs behind a provider plugin or a new
core trait. Domain models and persistence do not depend on an AI SDK, so the
base application remains offline-capable and deterministic.
