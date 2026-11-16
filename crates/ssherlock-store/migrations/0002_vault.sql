-- The vault (Phase 4, ADR 0008): which master key seals this database's secrets.
--
-- One row at most. `key_location` says where the key is kept, so a later start never makes a
-- new key just because the OS keychain is briefly unavailable. `check_value` is a known
-- plaintext sealed with the key, so a wrong key is caught before anything else is opened.

CREATE TABLE vault (
    id            INTEGER PRIMARY KEY NOT NULL CHECK (id = 1),
    vault_id      TEXT NOT NULL,
    key_location  TEXT NOT NULL CHECK (key_location IN ('keychain', 'file')),
    check_value   BLOB NOT NULL,
    created_at    TEXT NOT NULL
) STRICT;
