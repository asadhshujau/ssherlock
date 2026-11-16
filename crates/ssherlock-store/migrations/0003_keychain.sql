-- Saved keys and identities (Phase 4).
--
-- Secrets are BLOBs sealed by the vault (ADR 0008); nothing here is plaintext. Hosts and groups
-- point at keys and identities from their JSON `settings`, so those references are cleared by
-- the store when a key or identity is deleted rather than by a foreign key.

CREATE TABLE keys (
    id           TEXT PRIMARY KEY NOT NULL,
    label        TEXT NOT NULL DEFAULT '',
    kind         TEXT NOT NULL,
    public_key   TEXT NOT NULL,
    fingerprint  TEXT NOT NULL,
    private_key  BLOB NOT NULL,
    -- 'unencrypted', 'ask' (encrypted, passphrase asked for) or 'saved' (passphrase sealed below).
    protection   TEXT NOT NULL CHECK (protection IN ('unencrypted', 'ask', 'saved')),
    passphrase   BLOB,
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL,
    CHECK ((protection = 'saved') = (passphrase IS NOT NULL))
) STRICT;

CREATE TABLE identities (
    id          TEXT PRIMARY KEY NOT NULL,
    label       TEXT NOT NULL DEFAULT '',
    username    TEXT NOT NULL,
    password    BLOB,
    key_id      TEXT REFERENCES keys (id) ON DELETE SET NULL,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
) STRICT;

CREATE INDEX identities_key ON identities (key_id);
