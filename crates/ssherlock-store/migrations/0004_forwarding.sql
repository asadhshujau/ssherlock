-- Port-forwarding rules saved per host (Phase 5).
--
-- A rule belongs to one host and goes when the host is deleted for good. Moving a host to the
-- trash copies its rules into the trash entry first, so restoring brings them back.

CREATE TABLE forward_rules (
    id           TEXT PRIMARY KEY NOT NULL,
    host_id      TEXT NOT NULL REFERENCES hosts (id) ON DELETE CASCADE,
    label        TEXT NOT NULL DEFAULT '',
    -- 'local' (ssh -L), 'remote' (ssh -R) or 'dynamic' (ssh -D, a SOCKS proxy).
    kind         TEXT NOT NULL CHECK (kind IN ('local', 'remote', 'dynamic')),
    -- 'localhost' (loopback only) or 'all' (every interface).
    bind         TEXT NOT NULL CHECK (bind IN ('localhost', 'all')),
    port         INTEGER NOT NULL CHECK (port BETWEEN 1 AND 65535),
    -- Where connections go; a SOCKS proxy has no fixed destination.
    target_host  TEXT,
    target_port  INTEGER CHECK (target_port BETWEEN 1 AND 65535),
    is_enabled   INTEGER NOT NULL DEFAULT 1 CHECK (is_enabled IN (0, 1)),
    auto_start   INTEGER NOT NULL DEFAULT 0 CHECK (auto_start IN (0, 1)),
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL,
    CHECK ((kind = 'dynamic') = (target_host IS NULL)),
    CHECK ((target_host IS NULL) = (target_port IS NULL))
) STRICT;

CREATE INDEX forward_rules_host ON forward_rules (host_id);
