-- Room mappings: Matrix room <-> external platform room.
CREATE TABLE IF NOT EXISTS room_mappings (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    matrix_room_id TEXT NOT NULL,
    platform_id TEXT NOT NULL,
    external_room_id TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(matrix_room_id, platform_id),
    UNIQUE(platform_id, external_room_id)
);

-- Message mappings: Matrix event <-> external message (per platform).
CREATE TABLE IF NOT EXISTS message_mappings (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    matrix_event_id TEXT NOT NULL,
    platform_id TEXT NOT NULL,
    external_message_id TEXT NOT NULL,
    room_mapping_id INTEGER NOT NULL REFERENCES room_mappings(id) ON DELETE CASCADE,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(matrix_event_id, platform_id),
    UNIQUE(platform_id, external_message_id)
);

-- Puppet users: bridged external users represented as Matrix users.
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

-- Webhooks: outbound HTTP callbacks for message delivery.
-- forward_sources: allowlist of source platforms.
--   Empty (default) = deny all, "*" = forward all, "telegram,matrix" = specific.
-- capabilities: comma-separated list of supported features.
--   e.g. "message,image,reaction,edit,redaction,command"
-- owner: Matrix user ID of the integration operator. Auto-invited into
--   portal rooms created for this platform.
CREATE TABLE IF NOT EXISTS webhooks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    platform_id TEXT NOT NULL,
    webhook_url TEXT NOT NULL,
    secret TEXT,
    events TEXT NOT NULL DEFAULT 'message',
    enabled INTEGER NOT NULL DEFAULT 1,
    forward_sources TEXT NOT NULL DEFAULT '',
    capabilities TEXT NOT NULL DEFAULT '',
    owner TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(platform_id, webhook_url)
);

-- Platform spaces: one Matrix Space per platform for organizing bridged rooms.
CREATE TABLE IF NOT EXISTS platform_spaces (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    platform_id TEXT NOT NULL UNIQUE,
    matrix_space_id TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Indexes.
CREATE INDEX IF NOT EXISTS idx_room_mappings_matrix ON room_mappings(matrix_room_id);
CREATE INDEX IF NOT EXISTS idx_room_mappings_external ON room_mappings(platform_id, external_room_id);
CREATE INDEX IF NOT EXISTS idx_message_mappings_matrix ON message_mappings(matrix_event_id);
CREATE INDEX IF NOT EXISTS idx_message_mappings_external ON message_mappings(platform_id, external_message_id);
CREATE INDEX IF NOT EXISTS idx_puppets_external ON puppets(platform_id, external_user_id);
CREATE INDEX IF NOT EXISTS idx_webhooks_platform ON webhooks(platform_id, enabled);
CREATE INDEX IF NOT EXISTS idx_platform_spaces_platform ON platform_spaces(platform_id);
