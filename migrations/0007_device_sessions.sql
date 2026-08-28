-- Per-device session tokens. Each device gets a unique opaque token issued
-- via the bootstrap (static API) token. The token itself is never stored —
-- only its SHA-256 digest, so a database leak does not expose live sessions.
-- Tokens are long-lived (loopback/Tailscale boundary) and revocable.
CREATE TABLE device_sessions (
    id UUID PRIMARY KEY,
    token_hash TEXT NOT NULL UNIQUE,
    label TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    revoked_at TIMESTAMPTZ,
    last_used_at TIMESTAMPTZ
);

CREATE INDEX device_sessions_token_hash_idx ON device_sessions (token_hash) WHERE revoked_at IS NULL;
