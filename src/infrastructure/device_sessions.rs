use chrono::{DateTime, Utc};
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum DeviceSessionError {
    #[error("database error: {0}")]
    Database(String),
}

/// A device session row, as returned to the API layer. The token digest is
/// never included in serialized responses — only metadata for listing.
#[derive(Debug, Clone, Serialize)]
pub struct DeviceSessionRow {
    pub id: Uuid,
    pub label: String,
    pub created_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
}

/// Database access for per-device session tokens. The raw token is returned
/// to the caller only at creation time; thereafter lookups are by digest.
#[derive(Debug, Clone)]
pub struct DeviceSessionStore {
    pool: PgPool,
}

impl DeviceSessionStore {
    #[must_use]
    pub const fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Issues a new device session. Returns the raw opaque token (shown to
    /// the caller once) and the persisted row metadata. The stored digest is
    /// SHA-256 of the raw token, so a DB leak does not expose live sessions.
    pub async fn issue(
        &self,
        label: &str,
    ) -> Result<(String, DeviceSessionRow), DeviceSessionError> {
        let token = generate_token();
        let hash = hash_token(&token);
        let id = Uuid::new_v4();

        sqlx::query(
            r#"
            INSERT INTO device_sessions (id, token_hash, label)
            VALUES ($1, $2, $3)
            "#,
        )
        .bind(id)
        .bind(&hash)
        .bind(label)
        .execute(&self.pool)
        .await
        .map_err(|e| DeviceSessionError::Database(e.to_string()))?;

        let row = sqlx::query(
            r#"
            SELECT id, label, created_at, revoked_at, last_used_at
              FROM device_sessions
             WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| DeviceSessionError::Database(e.to_string()))?;

        Ok((token, row_to_session(row)))
    }

    /// Validates a raw token against active (non-revoked) sessions. Returns
    /// the session row if valid, and updates `last_used_at`. Returns `None`
    /// for unknown or revoked tokens — the caller treats both as unauthorized.
    pub async fn validate(
        &self,
        raw_token: &str,
    ) -> Result<Option<DeviceSessionRow>, DeviceSessionError> {
        let hash = hash_token(raw_token);

        let row = sqlx::query(
            r#"
            UPDATE device_sessions
               SET last_used_at = now()
             WHERE token_hash = $1
               AND revoked_at IS NULL
             RETURNING id, label, created_at, revoked_at, last_used_at
            "#,
        )
        .bind(&hash)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| DeviceSessionError::Database(e.to_string()))?;

        Ok(row.map(row_to_session))
    }

    /// Lists all device sessions, newest first.
    pub async fn list(&self) -> Result<Vec<DeviceSessionRow>, DeviceSessionError> {
        let rows = sqlx::query(
            r#"
            SELECT id, label, created_at, revoked_at, last_used_at
              FROM device_sessions
             ORDER BY created_at DESC
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| DeviceSessionError::Database(e.to_string()))?;

        Ok(rows.into_iter().map(row_to_session).collect())
    }

    /// Revokes a device session by id. Idempotent — revoking an already-
    /// revoked session is a no-op. Returns whether a session was found.
    pub async fn revoke(&self, id: Uuid) -> Result<bool, DeviceSessionError> {
        let affected = sqlx::query(
            r#"
            UPDATE device_sessions
               SET revoked_at = now()
             WHERE id = $1 AND revoked_at IS NULL
            "#,
        )
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(|e| DeviceSessionError::Database(e.to_string()))?
        .rows_affected();

        Ok(affected > 0)
    }
}

fn row_to_session(row: sqlx::postgres::PgRow) -> DeviceSessionRow {
    DeviceSessionRow {
        id: row.get("id"),
        label: row.get("label"),
        created_at: row.get("created_at"),
        revoked_at: row.get("revoked_at"),
        last_used_at: row.get("last_used_at"),
    }
}

/// Generates a random opaque token from two concatenated UUID v4 values
/// (256 bits of entropy), with dashes stripped. This avoids pulling in a
/// separate RNG or base64 dependency — `uuid` v4 is already a dependency.
fn generate_token() -> String {
    let mut token = Uuid::new_v4().to_string();
    token.push_str(&Uuid::new_v4().to_string());
    token.retain(|c| c != '-');
    token
}

/// SHA-256 digest of the raw token, hex-encoded. Stored in the database so
/// a leak never exposes live sessions. The digest has a fixed length, so
/// constant-time comparison in the auth layer never branches on token length.
fn hash_token(raw_token: &str) -> String {
    let digest = Sha256::digest(raw_token.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_tokens_are_unique() {
        let a = generate_token();
        let b = generate_token();
        assert_ne!(a, b);
        assert_eq!(a.len(), 64); // 2 × 32 hex chars from UUID v4
    }

    #[test]
    fn hash_is_deterministic_and_fixed_length() {
        let h1 = hash_token("test-token");
        let h2 = hash_token("test-token");
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64); // SHA-256 hex = 64 chars
    }
}
