-- Revert: drop the notes table.

DROP TRIGGER IF EXISTS notes_touch_trg ON notes;
DROP FUNCTION IF EXISTS notes_touch();
DROP TABLE IF EXISTS notes;
