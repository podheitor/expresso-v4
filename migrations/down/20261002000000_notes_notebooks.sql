-- Revert: detach notes and drop the notebooks table.

ALTER TABLE notes DROP COLUMN IF EXISTS notebook_id;
DROP TABLE IF EXISTS notes_notebooks;
