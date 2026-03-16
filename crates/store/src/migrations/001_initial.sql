CREATE TABLE IF NOT EXISTS room_mappings (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    matrix_room_id TEXT NOT NULL,
    platform_id TEXT NOT NULL,
    external_room_id TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(matrix_room_id, platform_id),
    UNIQUE(platform_id, external_room_id)
);

CREATE TABLE IF NOT EXISTS message_mappings (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    matrix_event_id TEXT NOT NULL,
    platform_id TEXT NOT NULL,
    external_message_id TEXT NOT NULL,
    room_mapping_id INTEGER NOT NULL REFERENCES room_mappings(id),
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(matrix_event_id),
    UNIQUE(platform_id, external_message_id)
);

CREATE TABLE IF NOT EXISTS puppets (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    matrix_user_id TEXT NOT NULL UNIQUE,
    platform_id TEXT NOT NULL,
    external_user_id TEXT NOT NULL,
    display_name TEXT,
    avatar_mxc TEXT,
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(platform_id, external_user_id)
);

CREATE INDEX IF NOT EXISTS idx_room_mappings_matrix ON room_mappings(matrix_room_id);
CREATE INDEX IF NOT EXISTS idx_room_mappings_external ON room_mappings(platform_id, external_room_id);
CREATE INDEX IF NOT EXISTS idx_message_mappings_external ON message_mappings(platform_id, external_message_id);
CREATE INDEX IF NOT EXISTS idx_puppets_external ON puppets(platform_id, external_user_id);
