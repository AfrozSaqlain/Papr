# Contributing

Use stable Rust and keep changes scoped to the owning crate. New database
changes require an append-only migration and an in-memory migration test.
External I/O must not occur in ratatui render functions.

Before submitting:

    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets --all-features -- -D warnings
    cargo test --workspace
    cargo doc --workspace --no-deps

Do not introduce unwrap, expect, deliberate panics, or unsafe code in
production paths. Bug fixes should include a regression test.
