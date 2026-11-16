-- Saved hosts, the groups they're in, their tags, and the trash (Phase 3).
--
-- Ids are UUID v7 text, timestamps RFC 3339 UTC text. `settings` columns hold HostSettings as
-- JSON, so settings added by later phases need no migration (their fields are serde(default)).

CREATE TABLE groups (
    id          TEXT PRIMARY KEY NOT NULL,
    name        TEXT NOT NULL,
    -- Deleting a group for good moves what was inside it to the top level. The app normally
    -- deletes through the trash, which takes the inside along.
    parent_id   TEXT REFERENCES groups (id) ON DELETE SET NULL,
    settings    TEXT NOT NULL DEFAULT '{}',
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
) STRICT;

CREATE INDEX groups_parent ON groups (parent_id);

CREATE TABLE hosts (
    id                 TEXT PRIMARY KEY NOT NULL,
    label              TEXT NOT NULL DEFAULT '',
    address            TEXT NOT NULL,
    group_id           TEXT REFERENCES groups (id) ON DELETE SET NULL,
    notes              TEXT NOT NULL DEFAULT '',
    is_favourite       INTEGER NOT NULL DEFAULT 0 CHECK (is_favourite IN (0, 1)),
    settings           TEXT NOT NULL DEFAULT '{}',
    created_at         TEXT NOT NULL,
    updated_at         TEXT NOT NULL,
    last_connected_at  TEXT
) STRICT;

CREATE INDEX hosts_group ON hosts (group_id);

CREATE TABLE host_tags (
    host_id  TEXT NOT NULL REFERENCES hosts (id) ON DELETE CASCADE,
    tag      TEXT NOT NULL,
    PRIMARY KEY (host_id, tag)
) STRICT, WITHOUT ROWID;

CREATE INDEX host_tags_tag ON host_tags (tag);

CREATE TABLE trash (
    id          TEXT PRIMARY KEY NOT NULL,
    deleted_at  TEXT NOT NULL,
    -- TrashContents as JSON: full copies of the deleted records.
    contents    TEXT NOT NULL
) STRICT;
