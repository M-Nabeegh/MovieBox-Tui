PRAGMA foreign_keys = ON;

CREATE TABLE users (
    id TEXT PRIMARY KEY NOT NULL,
    username TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE sessions (
    id TEXT PRIMARY KEY NOT NULL,
    user_id TEXT NOT NULL,
    token_hash BLOB NOT NULL,
    csrf_token TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    last_seen_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    absolute_expires_at INTEGER NOT NULL,
    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);

CREATE INDEX idx_sessions_expires_at ON sessions(expires_at);

CREATE TABLE jobs (
    id TEXT PRIMARY KEY NOT NULL,
    catalog_id TEXT NOT NULL,
    source_id TEXT NOT NULL,
    subtitle_id TEXT,
    title TEXT NOT NULL,
    year TEXT,
    media_type TEXT NOT NULL,
    season_number INTEGER,
    episode_number INTEGER,
    episode_title TEXT,
    requested_height INTEGER NOT NULL,
    state TEXT NOT NULL,
    final_video_path TEXT NOT NULL,
    final_subtitle_path TEXT,
    partial_video_path TEXT NOT NULL,
    partial_subtitle_path TEXT,
    downloaded_bytes INTEGER NOT NULL DEFAULT 0,
    total_bytes INTEGER,
    speed_bytes_per_second INTEGER,
    attempt INTEGER NOT NULL DEFAULT 0,
    error_code TEXT,
    error_message TEXT,
    warning TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    version INTEGER NOT NULL
);

CREATE INDEX idx_jobs_state_created_at ON jobs(state, created_at);

CREATE TABLE job_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,
    job_id TEXT NOT NULL,
    job_version INTEGER NOT NULL,
    kind TEXT NOT NULL,
    state TEXT NOT NULL,
    downloaded_bytes INTEGER NOT NULL,
    total_bytes INTEGER,
    speed_bytes_per_second INTEGER,
    attempt INTEGER NOT NULL,
    error_code TEXT,
    error_message TEXT,
    warning TEXT,
    created_at INTEGER NOT NULL,
    FOREIGN KEY (job_id) REFERENCES jobs(id) ON DELETE CASCADE
);

CREATE INDEX idx_job_events_job_id_id ON job_events(job_id, id);
