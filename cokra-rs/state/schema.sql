-- Cokra State Database Schema
CREATE TABLE IF NOT EXISTS threads (
    id TEXT PRIMARY KEY,
    name TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    archived INTEGER DEFAULT 0
);

CREATE TABLE IF NOT EXISTS rollouts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    thread_id TEXT NOT NULL,
    turn_id TEXT,
    event_type TEXT NOT NULL,
    data BLOB,
    created_at INTEGER NOT NULL,
    FOREIGN KEY (thread_id) REFERENCES threads(id)
);

CREATE INDEX IF NOT EXISTS idx_rollouts_thread ON rollouts(thread_id);
CREATE INDEX IF NOT EXISTS idx_threads_updated ON threads(updated_at);

