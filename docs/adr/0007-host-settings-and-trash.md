# 0007 — Host settings as JSON, the trash as snapshots

**Status:** accepted · 2026-11-16

## Context
Hosts and groups share a set of inheritable settings (port, user, colour, algorithms,
compression) that every later phase grows: identities (4), environment (8), terminal overrides
(9). Deleting must be undoable, and deleting a group takes everything inside it.

## Decision
Store `HostSettings` as one JSON `TEXT` column on `hosts` and `groups`, with `#[serde(default)]`
on every field, rather than a column per setting. Searchable and relational fields (label,
address, group, tags, timestamps) stay real columns. The trash is its own table: each entry is a
JSON snapshot of the records one deletion removed (a host, or a group's whole subtree with its
hosts), restored in one transaction, at the top level if the parent has gone since.

## Consequences
- New settings need no migration, and old databases load them as "inherit".
- SQL can't filter on a setting without `json_extract`; nothing needs that today.
- The live tables never hold half-deleted rows, and every query ignores the trash for free.
- A snapshot is only as readable as the model's serde shape, so model fields keep the
  `#[serde(default)]` rule (CLAUDE.md §8).
