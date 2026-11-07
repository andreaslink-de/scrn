-- Device groups, such as "Work" or "Home". Every API token belongs to one
-- group of its owner, and a screenshot remembers the token that uploaded
-- it, so the gallery can filter by group. Moving a token to another group
-- moves its screenshots along.
--
-- Each user has exactly one default group, which cannot be deleted. Its
-- name stays NULL until the user renames it; the UI then shows a
-- translated default name. Screenshots without a token (uploaded before
-- this migration) count as part of the default group.
CREATE TABLE device_groups (
    id         INTEGER PRIMARY KEY,
    user_id    INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    name       TEXT    CHECK (name IS NULL OR length(name) BETWEEN 1 AND 50),
    is_default INTEGER NOT NULL DEFAULT 0 CHECK (is_default IN (0, 1)),
    created_at INTEGER NOT NULL,
    -- Target of the composite key in api_tokens: a token can only belong
    -- to a group of its own user.
    UNIQUE (id, user_id),
    CHECK (name IS NOT NULL OR is_default = 1)
) STRICT;

CREATE INDEX device_groups_user_id ON device_groups (user_id);
CREATE UNIQUE INDEX device_groups_one_default ON device_groups (user_id) WHERE is_default = 1;
CREATE UNIQUE INDEX device_groups_user_name ON device_groups (user_id, name COLLATE NOCASE)
    WHERE name IS NOT NULL;

-- Every new user gets a default group.
CREATE TRIGGER users_default_device_group AFTER INSERT ON users
BEGIN
    INSERT INTO device_groups (user_id, is_default, created_at)
    VALUES (NEW.id, 1, NEW.created_at);
END;

INSERT INTO device_groups (user_id, is_default, created_at)
SELECT id, 1, created_at FROM users;

-- api_tokens gets a NOT NULL group, which needs a new table in SQLite.
-- Existing tokens go into the default group of their user.
CREATE TABLE api_tokens_new (
    id           INTEGER PRIMARY KEY,
    user_id      INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    group_id     INTEGER NOT NULL,
    name         TEXT    NOT NULL CHECK (length(name) BETWEEN 1 AND 100),
    token_hash   BLOB    NOT NULL UNIQUE CHECK (length(token_hash) = 32),
    created_at   INTEGER NOT NULL,
    last_used_at INTEGER,
    revoked_at   INTEGER,
    FOREIGN KEY (group_id, user_id) REFERENCES device_groups (id, user_id)
) STRICT;

INSERT INTO api_tokens_new
    (id, user_id, group_id, name, token_hash, created_at, last_used_at, revoked_at)
SELECT t.id, t.user_id, g.id, t.name, t.token_hash, t.created_at, t.last_used_at, t.revoked_at
FROM api_tokens t
JOIN device_groups g ON g.user_id = t.user_id AND g.is_default = 1;

DROP TABLE api_tokens;
ALTER TABLE api_tokens_new RENAME TO api_tokens;

CREATE INDEX api_tokens_user_id ON api_tokens (user_id);
CREATE INDEX api_tokens_group_id ON api_tokens (group_id, user_id);

-- The token that uploaded a screenshot.
ALTER TABLE screenshots ADD COLUMN token_id INTEGER REFERENCES api_tokens (id) ON DELETE SET NULL;
CREATE INDEX screenshots_token_id ON screenshots (token_id);
