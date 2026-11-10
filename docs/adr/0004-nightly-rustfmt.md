# 0004 — Nightly rustfmt for formatting only

**Status:** accepted · 2026-11-10

## Context
`CLAUDE.md` asks for `imports_granularity = "Module"` and `group_imports = "StdExternalCrate"` so
every file groups and merges imports the same way. Both options are unstable: stable rustfmt
prints a warning and ignores them, so files would be formatted inconsistently.

## Decision
Format with nightly rustfmt (`cargo +nightly fmt`, wrapped by `just fmt` / `just fmt-check` and
the CI `rustfmt` job). Building, linting and testing stay on stable.

## Consequences
- Contributors install one extra component (`rustup toolchain install nightly --profile minimal
  --component rustfmt`), listed in the README.
- Nightly rustfmt can change output between versions; if that causes churn, pin a nightly date in
  CI and the justfile.
- If the two options stabilise, switch back to stable rustfmt and retire this ADR.
