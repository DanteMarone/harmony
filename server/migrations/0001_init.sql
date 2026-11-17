-- Accounts, invites and login sessions.
-- No email, no real name, no ID: a username is all Harmony knows about a person.

CREATE TABLE users (
    id            INTEGER PRIMARY KEY,
    username      TEXT    NOT NULL UNIQUE COLLATE NOCASE,
    display_name  TEXT    NOT NULL,
    password_hash TEXT    NOT NULL,
    is_admin      INTEGER NOT NULL DEFAULT 0,
    created_at    INTEGER NOT NULL
);

-- An invite is a one-time code. Only its SHA-256 hash is stored.
CREATE TABLE invites (
    id          INTEGER PRIMARY KEY,
    code_hash   TEXT    NOT NULL UNIQUE,
    created_by  INTEGER REFERENCES users(id) ON DELETE SET NULL,
    grants_admin INTEGER NOT NULL DEFAULT 0,
    created_at  INTEGER NOT NULL,
    expires_at  INTEGER NOT NULL,
    used_by     INTEGER REFERENCES users(id) ON DELETE SET NULL,
    used_at     INTEGER
);

-- A session is a random token held in a cookie. Only its SHA-256 hash is stored.
CREATE TABLE sessions (
    id          INTEGER PRIMARY KEY,
    token_hash  TEXT    NOT NULL UNIQUE,
    user_id     INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at  INTEGER NOT NULL,
    expires_at  INTEGER NOT NULL
);

CREATE INDEX sessions_user ON sessions(user_id);
