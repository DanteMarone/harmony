-- Photos and files, passed along end-to-end encrypted.
--
-- The server is a short-term relay, not a file host. The sender's device
-- encrypts a file with a fresh key, uploads the ciphertext, and sends the key
-- inside an ordinary encrypted message. Each receiving device downloads the
-- ciphertext, keeps its own decrypted copy, and says so. When every device
-- that should get the file has it, or after a few days, the server deletes
-- its copy. The bytes live in `<data dir>/attachments/<id>`, never in SQLite.

CREATE TABLE attachments (
    id            TEXT    PRIMARY KEY,   -- random, unguessable
    chat_id       TEXT    NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
    uploader_user INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    uploader_device INTEGER NOT NULL,
    size          INTEGER NOT NULL,      -- of the ciphertext, in bytes
    created_at    INTEGER NOT NULL,
    expires_at    INTEGER NOT NULL
);
CREATE INDEX attachments_chat ON attachments(chat_id);
CREATE INDEX attachments_expiry ON attachments(expires_at);

-- The devices that have not picked a file up yet: every device of every
-- member at upload time, except the one that sent it.
CREATE TABLE attachment_pending (
    attachment_id TEXT    NOT NULL REFERENCES attachments(id) ON DELETE CASCADE,
    device_id     INTEGER NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    PRIMARY KEY (attachment_id, device_id)
) WITHOUT ROWID;
CREATE INDEX attachment_pending_device ON attachment_pending(device_id);
