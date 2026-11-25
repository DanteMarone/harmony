-- Rooms: named chats that every account can see and join on its own, each
-- with a voice channel that stays open for people to drop in and out of.
--
-- A room is stored as a group chat with this flag set. Joining one works
-- like being added to a group: another member's device hands the newcomer's
-- devices the room's keys, so room messages and voice stay end-to-end
-- encrypted. The room's name and who is in it are visible to the server, as
-- they are for group chats.
ALTER TABLE chats ADD COLUMN room INTEGER NOT NULL DEFAULT 0;
CREATE INDEX chats_room ON chats(room) WHERE room = 1;
