-- Device verification and encrypted history backup.
--
-- Each person has a recovery code that never leaves their devices. From it
-- they derive an identity key, whose public half is stored here, and a
-- backup key, which the server never sees.

-- The public identity key. Friends compare it (as a safety number) to be
-- sure they are talking to the right person.
ALTER TABLE users ADD COLUMN identity_key BLOB;

-- The owner's signature over this device's key, made with the identity key:
-- proof that the device belongs to them and was not slipped in by the server.
ALTER TABLE devices ADD COLUMN identity_signature BLOB;

-- A person's own copy of the messages they could read, encrypted on their
-- device with their backup key. Opaque to the server, like `events.data`.
CREATE TABLE backup_messages (
    user_id     INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    chat_id     TEXT    NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
    seq         INTEGER NOT NULL,   -- the event this is a copy of
    data        BLOB    NOT NULL,
    PRIMARY KEY (user_id, chat_id, seq)
) WITHOUT ROWID;
