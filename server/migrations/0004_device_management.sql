-- Device management: people can see their devices and sign one out remotely.

-- A label the owner chooses (by default the browser and system, such as
-- "Firefox on Windows"), so they can tell their devices apart.
ALTER TABLE devices ADD COLUMN name TEXT;
-- Rounded to the minute, and only shown to the device's owner.
ALTER TABLE devices ADD COLUMN last_seen_at INTEGER;
-- The login session the device last used. Signing the device out ends it.
ALTER TABLE devices ADD COLUMN session_id INTEGER REFERENCES sessions(id) ON DELETE SET NULL;
