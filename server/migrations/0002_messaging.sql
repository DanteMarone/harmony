-- End-to-end encrypted messaging.
--
-- The server never sees message content or keys. Everything in a `data`
-- column is opaque ciphertext produced on a user's device. What the server
-- does know is listed here in the open: who is in which chat, which device
-- sent an event, and when.

-- A device is one browser or app install. Each has its own encryption keys,
-- which never leave it.
CREATE TABLE devices (
    id          INTEGER PRIMARY KEY,
    user_id     INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at  INTEGER NOT NULL,
    -- Set when the device is signed out for good. Kept (not deleted) so the
    -- other members' devices know to remove it from each chat's key group.
    revoked_at  INTEGER
);
CREATE INDEX devices_user ON devices(user_id);

-- Public key packages: tokens that let others add a device to a chat while
-- it is offline. One-time packages are handed out once and deleted; the
-- last-resort package is reused only when the one-time ones run out.
CREATE TABLE key_packages (
    id          INTEGER PRIMARY KEY,
    device_id   INTEGER NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    last_resort INTEGER NOT NULL DEFAULT 0,
    data        BLOB    NOT NULL
);
CREATE INDEX key_packages_device ON key_packages(device_id, last_resort);

CREATE TABLE chats (
    id          TEXT    PRIMARY KEY,           -- random; also the encryption group id
    kind        TEXT    NOT NULL CHECK (kind IN ('dm', 'group')),
    name        TEXT,                          -- group chats only
    dm_key      TEXT    UNIQUE,                -- "lowId:highId" for DMs, so each pair has one
    epoch       INTEGER NOT NULL DEFAULT 0,    -- goes up by one on every key change
    created_by  INTEGER REFERENCES users(id) ON DELETE SET NULL,
    created_at  INTEGER NOT NULL
);

-- Who is meant to be in a chat. This is what access control uses.
CREATE TABLE chat_members (
    chat_id     TEXT    NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
    user_id     INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    joined_at   INTEGER NOT NULL,
    PRIMARY KEY (chat_id, user_id)
);
CREATE INDEX chat_members_user ON chat_members(user_id);

-- Which devices currently hold the chat's keys, as reported by the devices
-- that made each key change.
CREATE TABLE chat_devices (
    chat_id     TEXT    NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
    device_id   INTEGER NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    -- Events up to and including this one are not readable by the device.
    joined_seq  INTEGER NOT NULL DEFAULT 0,
    -- Set when the device reports it lost its keys for this chat. Another
    -- device then removes it from the key group and adds it back fresh.
    needs_rejoin INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (chat_id, device_id)
);

-- The ordered stream of a chat: encrypted messages and key changes.
CREATE TABLE events (
    seq           INTEGER PRIMARY KEY AUTOINCREMENT,
    chat_id       TEXT    NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
    kind          TEXT    NOT NULL CHECK (kind IN ('message', 'commit')),
    sender_user   INTEGER,
    sender_device INTEGER,
    epoch         INTEGER NOT NULL,   -- the epoch the event was encrypted in
    client_id     TEXT,               -- sender-chosen id, so a device can spot its own echo
    data          BLOB    NOT NULL,
    created_at    INTEGER NOT NULL
);
CREATE INDEX events_chat ON events(chat_id, seq);
-- Lets a device safely retry a send: the same message is never stored twice.
CREATE UNIQUE INDEX events_client ON events(chat_id, sender_device, client_id) WHERE client_id IS NOT NULL;

-- Encrypted invitations waiting for a device to come online and join a chat.
CREATE TABLE welcomes (
    id          INTEGER PRIMARY KEY,
    device_id   INTEGER NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    chat_id     TEXT    NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
    after_seq   INTEGER NOT NULL,   -- the device reads the chat from the event after this one
    data        BLOB    NOT NULL,
    created_at  INTEGER NOT NULL
);
CREATE INDEX welcomes_device ON welcomes(device_id);
