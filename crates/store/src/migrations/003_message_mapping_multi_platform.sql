-- Allow the same Matrix event to be mapped to multiple platforms.
-- Previously UNIQUE(matrix_event_id) prevented cross-platform forwarding
-- because one event maps to multiple external platforms simultaneously.

-- SQLite does not support DROP CONSTRAINT, so we recreate the table.
CREATE TABLE IF NOT EXISTS message_mappings_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    matrix_event_id TEXT NOT NULL,
    platform_id TEXT NOT NULL,
    external_message_id TEXT NOT NULL,
    room_mapping_id INTEGER NOT NULL REFERENCES room_mappings(id),
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(matrix_event_id, platform_id),
    UNIQUE(platform_id, external_message_id)
);

INSERT OR IGNORE INTO message_mappings_new (id, matrix_event_id, platform_id, external_message_id, room_mapping_id, created_at)
    SELECT id, matrix_event_id, platform_id, external_message_id, room_mapping_id, created_at
    FROM message_mappings;

DROP TABLE message_mappings;
ALTER TABLE message_mappings_new RENAME TO message_mappings;

CREATE INDEX IF NOT EXISTS idx_message_mappings_matrix ON message_mappings(matrix_event_id);
CREATE INDEX IF NOT EXISTS idx_message_mappings_external ON message_mappings(platform_id, external_message_id);
