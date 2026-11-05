-- Initial schema: users, invites, sessions, api_tokens, screenshots, passkeys.
--
-- Conventions:
-- * Timestamps are INTEGER Unix seconds (UTC).
-- * Hashes (SHA-256) are 32-byte BLOBs. Secrets are never stored in clear text.
-- * Foreign keys cascade on delete (PRAGMA foreign_keys is enabled per connection).

CREATE TABLE users (
    id                  INTEGER PRIMARY KEY,
    username            TEXT    NOT NULL UNIQUE COLLATE NOCASE
                                CHECK (length(username) BETWEEN 1 AND 64),
    -- argon2id PHC string. NULL once passkeys allow password-less accounts.
    password_hash       TEXT,
    role                TEXT    NOT NULL DEFAULT 'user'
                                CHECK (role IN ('admin', 'user')),
    language            TEXT    NOT NULL DEFAULT 'de'
                                CHECK (language IN ('de', 'en')),
    theme               TEXT    NOT NULL DEFAULT 'dark'
                                CHECK (theme IN ('dark', 'light')),
    -- Default expiry of new public links in days, 0 means never.
    default_expiry_days INTEGER NOT NULL DEFAULT 0
                                CHECK (default_expiry_days >= 0),
    -- NULL means the server default (SCRN_DEFAULT_QUOTA_MB), 0 means unlimited.
    quota_bytes         INTEGER CHECK (quota_bytes IS NULL OR quota_bytes >= 0),
    created_at          INTEGER NOT NULL,
    disabled_at         INTEGER
) STRICT;

CREATE TABLE invites (
    id          INTEGER PRIMARY KEY,
    token_hash  BLOB    NOT NULL UNIQUE CHECK (length(token_hash) = 32),
    created_by  INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    created_at  INTEGER NOT NULL,
    expires_at  INTEGER NOT NULL,
    used_by     INTEGER REFERENCES users (id) ON DELETE CASCADE,
    used_at     INTEGER,
    CHECK ((used_by IS NULL) = (used_at IS NULL))
) STRICT;

CREATE INDEX invites_created_by ON invites (created_by);
CREATE INDEX invites_used_by ON invites (used_by);
CREATE INDEX invites_expires_at ON invites (expires_at);

CREATE TABLE sessions (
    id_hash      BLOB    PRIMARY KEY CHECK (length(id_hash) = 32),
    user_id      INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    created_at   INTEGER NOT NULL,
    last_seen_at INTEGER NOT NULL,
    expires_at   INTEGER NOT NULL
) STRICT, WITHOUT ROWID;

CREATE INDEX sessions_user_id ON sessions (user_id);
CREATE INDEX sessions_expires_at ON sessions (expires_at);

CREATE TABLE api_tokens (
    id           INTEGER PRIMARY KEY,
    user_id      INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    name         TEXT    NOT NULL CHECK (length(name) BETWEEN 1 AND 100),
    token_hash   BLOB    NOT NULL UNIQUE CHECK (length(token_hash) = 32),
    created_at   INTEGER NOT NULL,
    last_used_at INTEGER,
    revoked_at   INTEGER
) STRICT;

CREATE INDEX api_tokens_user_id ON api_tokens (user_id);

CREATE TABLE screenshots (
    -- UUID in hyphenated lowercase form; the file is /data/files/<id>.png.
    id          TEXT    PRIMARY KEY CHECK (length(id) = 36),
    user_id     INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    visibility  TEXT    NOT NULL DEFAULT 'private'
                        CHECK (visibility IN ('private', 'public')),
    -- Short link ID. Set if and only if the screenshot is public.
    short_id    TEXT,
    width       INTEGER NOT NULL CHECK (width > 0),
    height      INTEGER NOT NULL CHECK (height > 0),
    bytes       INTEGER NOT NULL CHECK (bytes >= 0),
    thumb_bytes INTEGER NOT NULL CHECK (thumb_bytes >= 0),
    sha256      BLOB    NOT NULL CHECK (length(sha256) = 32),
    created_at  INTEGER NOT NULL,
    -- Expiry of the public link; NULL means never. Only for public screenshots.
    expires_at  INTEGER,
    CHECK ((short_id IS NOT NULL) = (visibility = 'public')),
    CHECK (expires_at IS NULL OR visibility = 'public')
) STRICT, WITHOUT ROWID;

CREATE UNIQUE INDEX screenshots_short_id ON screenshots (short_id) WHERE short_id IS NOT NULL;
CREATE INDEX screenshots_user_id_created_at ON screenshots (user_id, created_at);
CREATE INDEX screenshots_expires_at ON screenshots (expires_at) WHERE expires_at IS NOT NULL;

CREATE TABLE passkeys (
    id            INTEGER PRIMARY KEY,
    user_id       INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    name          TEXT    NOT NULL CHECK (length(name) BETWEEN 1 AND 100),
    -- WebAuthn credential ID, used to look up the passkey during login.
    credential_id BLOB    NOT NULL UNIQUE,
    -- Serialized credential (public key, counter) as stored by webauthn-rs.
    credential    TEXT    NOT NULL,
    created_at    INTEGER NOT NULL,
    last_used_at  INTEGER
) STRICT;

CREATE INDEX passkeys_user_id ON passkeys (user_id);
