CREATE TABLE IF NOT EXISTS webhooks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    platform_id TEXT NOT NULL,
    webhook_url TEXT NOT NULL,
    secret TEXT,
    events TEXT NOT NULL DEFAULT 'message',
    enabled INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(platform_id, webhook_url)
);

CREATE INDEX IF NOT EXISTS idx_webhooks_platform ON webhooks(platform_id, enabled);
