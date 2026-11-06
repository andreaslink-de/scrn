-- Number of times the current short link of a screenshot was opened.
-- Reset to 0 whenever the link changes or goes away.
ALTER TABLE screenshots ADD COLUMN views INTEGER NOT NULL DEFAULT 0 CHECK (views >= 0);
