-- Single-use links that set a new password, created by an admin on the
-- admin page or with `scrn-server reset-link <username>`.
CREATE TABLE password_resets (
    id          INTEGER PRIMARY KEY,
    token_hash  BLOB    NOT NULL UNIQUE CHECK (length(token_hash) = 32),
    user_id     INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    -- NULL when created on the command line.
    created_by  INTEGER REFERENCES users (id) ON DELETE SET NULL,
    created_at  INTEGER NOT NULL,
    expires_at  INTEGER NOT NULL,
    used_at     INTEGER
) STRICT;

CREATE INDEX password_resets_user_id ON password_resets (user_id);
CREATE INDEX password_resets_created_by ON password_resets (created_by);

-- Last time the user of a session proved their identity with a passkey,
-- either at login or by confirming on the settings page. Within a short
-- window this replaces the current password when setting a new one.
ALTER TABLE sessions ADD COLUMN passkey_at INTEGER;
