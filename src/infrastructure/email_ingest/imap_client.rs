use chrono::{DateTime, Datelike, Utc};
use imap::{ClientBuilder, Session};
use mail_parser::{MessageParser, MimeHeaders};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use thiserror::Error;
use tracing::{info, warn};

/// A PDF attachment pulled from a sent email, plus the email metadata needed
/// for the audit trail.
#[derive(Debug, Clone)]
pub struct EmailAttachment {
    pub filename: String,
    pub content_hash: String,
    pub bytes: Vec<u8>,
    pub email_date: DateTime<Utc>,
    pub email_subject: String,
    pub email_recipient: String,
}

#[derive(Debug, Error)]
pub enum ImapFetchError {
    #[error("IMAP connection failed: {0}")]
    Connect(String),
    #[error("IMAP authentication failed: {0}")]
    Auth(String),
    #[error("IMAP mailbox error: {0}")]
    Mailbox(String),
    #[error("IMAP fetch error: {0}")]
    Fetch(String),
}

pub struct ImapConfig {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub sent_folder: String,
    pub recipient_filter: String,
    pub subject_filter: String,
    /// How many days back to search. Replaces the old current-month-only
    /// filter so historical invoices are captured on the first run.
    pub lookback_days: u32,
}

/// Size of each IMAP body fetch batch. Fetching 969 emails one-by-one is
/// 969 round-trips; batching into groups of 50 cuts that to ~20.
const FETCH_BATCH_SIZE: usize = 50;

/// Connects to the IMAP server, searches the SENT folder for emails matching
/// the recipient and subject filter, and extracts PDF attachments. The IMAP
/// crate is synchronous, so the whole session runs inside `spawn_blocking`.
///
/// Processing is batched: we first fetch only headers to filter by subject
/// (cheap), then fetch full bodies in batches of `FETCH_BATCH_SIZE`, hashing
/// and deduping as we go. This avoids loading all 969 emails into memory at
/// once and gives us incremental progress.
pub fn fetch_pdf_attachments(
    config: &ImapConfig,
    known_hashes: &HashSet<String>,
) -> Result<Vec<EmailAttachment>, ImapFetchError> {
    let client = ClientBuilder::new(&config.host, config.port)
        .connect()
        .map_err(|e| ImapFetchError::Connect(e.to_string()))?;

    let mut session = client
        .login(&config.username, &config.password)
        .map_err(|(e, _)| ImapFetchError::Auth(e.to_string()))?;

    let attachments = search_and_extract(&mut session, config, known_hashes);

    session
        .logout()
        .unwrap_or_else(|e| warn!("IMAP logout failed: {e}"));

    attachments
}

fn search_and_extract(
    session: &mut Session<imap::Connection>,
    config: &ImapConfig,
    known_hashes: &HashSet<String>,
) -> Result<Vec<EmailAttachment>, ImapFetchError> {
    session
        .select(&config.sent_folder)
        .map_err(|e| ImapFetchError::Mailbox(e.to_string()))?;

    // Search for emails TO the recipient filter, SINCE N days ago. The
    // lookback period (default 365 days) ensures historical invoices are
    // captured on the first run; the content-hash dedup table prevents
    // reprocessing on subsequent runs. IMAP date format is DD-Mon-YYYY.
    const MONTH_NAMES: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let now = Utc::now();
    let since_date = now - chrono::Duration::days(config.lookback_days as i64);
    let month_name = MONTH_NAMES[(since_date.month() as usize) - 1];
    let search_query = format!(
        "TO \"{}\" SINCE {:02}-{}-{}",
        config.recipient_filter,
        since_date.day(),
        month_name,
        since_date.year()
    );
    let uids = session
        .uid_search(&search_query)
        .map_err(|e| ImapFetchError::Fetch(e.to_string()))?;

    info!(
        "email ingest: {} UIDs match TO + SINCE {}-{:02}-{:02} filter ({}-day lookback)",
        uids.len(),
        since_date.year(),
        since_date.month(),
        since_date.day(),
        config.lookback_days,
    );

    // Sort UIDs and process in batches. We fetch full bodies (BODY.PEEK[])
    // because the imap crate doesn't populate `.body()` for partial section
    // fetches like HEADER.FIELDS — the body is only available for full
    // BODY[] fetches.
    let mut uids_vec: Vec<u32> = uids.into_iter().collect();
    uids_vec.sort_unstable();

    let mut attachments = Vec::new();
    let mut processed = 0;
    let total = uids_vec.len();

    for chunk in uids_vec.chunks(FETCH_BATCH_SIZE) {
        let uid_set = chunk
            .iter()
            .map(|u| u.to_string())
            .collect::<Vec<_>>()
            .join(",");

        let messages = match session.uid_fetch(&uid_set, "(BODY.PEEK[] INTERNALDATE)") {
            Ok(m) => m,
            Err(e) => {
                warn!("Body fetch failed for batch: {e}");
                continue;
            }
        };

        for message in messages.iter() {
            processed += 1;
            let raw = match message.body() {
                Some(body) => body,
                None => continue,
            };

            let parsed = MessageParser::default().parse(raw);
            let msg = match parsed {
                Some(m) => m,
                None => continue,
            };

            let subject = msg.subject().unwrap_or("").to_owned();

            // Client-side subject filter.
            if !subject.contains(&config.subject_filter) {
                continue;
            }

            let email_date = msg
                .date()
                .and_then(|d| chrono::DateTime::from_timestamp(d.to_timestamp(), 0))
                .unwrap_or_else(Utc::now);

            let recipient = config.recipient_filter.clone();

            for attachment in msg.attachments() {
                let filename = attachment.attachment_name().unwrap_or("unknown").to_owned();

                if !filename.to_ascii_lowercase().ends_with(".pdf") {
                    continue;
                }

                let bytes = attachment.contents();
                let mut hasher = Sha256::new();
                hasher.update(bytes);
                let hash = hasher
                    .finalize()
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>();

                if known_hashes.contains(&hash) {
                    continue;
                }

                attachments.push(EmailAttachment {
                    filename,
                    content_hash: hash,
                    bytes: bytes.to_vec(),
                    email_date,
                    email_subject: subject.clone(),
                    email_recipient: recipient.clone(),
                });
            }
        }

        info!(
            "email ingest: processed {processed}/{total} emails, {count} new attachments so far",
            count = attachments.len()
        );
    }

    Ok(attachments)
}
