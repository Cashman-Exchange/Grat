use crate::error::{ArchiveErrorKind, GratResult};
use crate::network::NetworkConfig;
use stellar_xdr::curr::{LedgerHeader, LedgerHeaderHistoryEntry, Limits, ReadXdr};

pub struct ArchiveClient {
    #[allow(dead_code)]
    client: reqwest::Client,

    #[allow(dead_code)]
    archive_urls: Vec<String>,
}

#[derive(Debug)]
pub struct ArchiveCheckpoint {
    pub ledger_sequence: u32,

    pub ledger_header: Vec<u8>,

    pub transaction_set: Vec<u8>,

    pub transaction_results: Vec<u8>,
}

impl ArchiveClient {
    pub fn new(config: &NetworkConfig) -> Self {
        Self {
            client: reqwest::Client::new(),
            archive_urls: config.archive_urls.clone(),
        }
    }

    pub async fn fetch_checkpoint(&self, ledger_sequence: u32) -> GratResult<ArchiveCheckpoint> {
        let checkpoint_seq = (ledger_sequence / 64) * 64;
        let _path = format_checkpoint_path(checkpoint_seq);
        let archive_count = self.archive_urls.len();
        let _ = &self.client;

        tracing::info!(
            archive_count,
            "Fetching archive checkpoint for ledger {checkpoint_seq}"
        );

        Err(ArchiveErrorKind::FetchFailed {
            file: format!("checkpoint-{checkpoint_seq}"),
            reason: "Archive fetch not yet implemented".to_string(),
        }
        .into())
    }

    pub async fn fetch_ledger_entry(
        &self,
        _ledger_sequence: u32,
        _key: &str,
    ) -> GratResult<Vec<u8>> {
        Err(ArchiveErrorKind::FetchFailed {
            file: format!("ledger-entry-{_ledger_sequence}-{_key}"),
            reason: "Ledger entry fetch not yet implemented".to_string(),
        }
        .into())
    }

    pub async fn get_ledger_header(&self, ledger_sequence: u32) -> GratResult<LedgerHeader> {
        let checkpoint = self.fetch_checkpoint(ledger_sequence).await?;
        let checkpoint_file = format!("checkpoint-{}", (ledger_sequence / 64) * 64);
        let entries = parse_ledger_header_stream(&checkpoint.ledger_header, &checkpoint_file)?;

        for entry in entries {
            if entry.header.ledger_seq == ledger_sequence {
                return Ok(entry.header);
            }
        }

        Err(ArchiveErrorKind::FetchFailed {
            file: checkpoint_file,
            reason: format!("ledger sequence {ledger_sequence} not found in checkpoint stream"),
        }
        .into())
    }
}

/// Parses a stream of XDR-encoded `LedgerHeaderHistoryEntry` records using RFC 5531 Record Marking format.
///
/// Each record fragment is preceded by a 4-byte header:
/// - Highest bit (0x8000_0000): 1 if this fragment is the final fragment of a record, 0 otherwise.
/// - Lower 31 bits (0x7FFF_FFFF): length of the fragment payload in bytes.
pub fn parse_ledger_header_stream(
    bytes: &[u8],
    file_name: &str,
) -> GratResult<Vec<LedgerHeaderHistoryEntry>> {
    let mut cursor = bytes;
    let mut entries = Vec::new();
    let mut record_buf = Vec::new();

    while !cursor.is_empty() {
        if cursor.len() < 4 {
            return Err(ArchiveErrorKind::MalformedXdr {
                file: file_name.to_string(),
                reason: format!(
                    "truncated record header: expected 4 bytes, found {}",
                    cursor.len()
                ),
            }
            .into());
        }

        let header = u32::from_be_bytes(cursor[..4].try_into().unwrap());
        cursor = &cursor[4..];

        let is_last = (header & 0x8000_0000) != 0;
        let fragment_len = (header & 0x7FFF_FFFF) as usize;

        if fragment_len > cursor.len() {
            return Err(ArchiveErrorKind::MalformedXdr {
                file: file_name.to_string(),
                reason: format!(
                    "truncated fragment payload: expected {} bytes, found {}",
                    fragment_len,
                    cursor.len()
                ),
            }
            .into());
        }

        let (fragment_bytes, rest) = cursor.split_at(fragment_len);
        cursor = rest;

        if is_last && record_buf.is_empty() {
            let entry = LedgerHeaderHistoryEntry::from_xdr(fragment_bytes, Limits::none())
                .map_err(|e| ArchiveErrorKind::MalformedXdr {
                    file: file_name.to_string(),
                    reason: format!("failed to decode LedgerHeaderHistoryEntry: {e}"),
                })?;
            entries.push(entry);
        } else {
            record_buf.extend_from_slice(fragment_bytes);

            if is_last {
                let entry = LedgerHeaderHistoryEntry::from_xdr(&record_buf, Limits::none())
                    .map_err(|e| ArchiveErrorKind::MalformedXdr {
                        file: file_name.to_string(),
                        reason: format!("failed to decode LedgerHeaderHistoryEntry: {e}"),
                    })?;
                entries.push(entry);
                record_buf.clear();
            }
        }
    }

    if !record_buf.is_empty() {
        return Err(ArchiveErrorKind::MalformedXdr {
            file: file_name.to_string(),
            reason: "unterminated record fragment stream".to_string(),
        }
        .into());
    }

    Ok(entries)
}

fn format_checkpoint_path(checkpoint_seq: u32) -> String {
    let hex = format!("{checkpoint_seq:08x}");
    format!(
        "{}/{}/{}/ledger-{}.xdr.gz",
        &hex[0..2],
        &hex[2..4],
        &hex[4..6],
        hex
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::GratError;
    use stellar_xdr::curr::{Hash, LedgerHeaderExt, LedgerHeaderHistoryEntryExt, WriteXdr};

    fn make_test_entry(ledger_seq: u32) -> LedgerHeaderHistoryEntry {
        let mut entry = LedgerHeaderHistoryEntry::default();
        entry.header.ledger_seq = ledger_seq;
        entry
    }

    fn frame_record(payload: &[u8], is_last: bool) -> Vec<u8> {
        let mut header_val = payload.len() as u32 & 0x7FFF_FFFF;
        if is_last {
            header_val |= 0x8000_0000;
        }
        let mut out = header_val.to_be_bytes().to_vec();
        out.extend_from_slice(payload);
        out
    }

    #[test]
    fn test_checkpoint_path_format() {
        let path = format_checkpoint_path(64);
        assert!(path.contains("ledger-"));
        assert!(path.ends_with(".xdr.gz"));
    }

    #[test]
    fn test_single_record() {
        let entry = make_test_entry(100);
        let xdr_bytes = entry.to_xdr(Limits::none()).unwrap();
        let framed = frame_record(&xdr_bytes, true);

        let parsed = parse_ledger_header_stream(&framed, "test.xdr").unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].header.ledger_seq, 100);
    }

    #[test]
    fn test_multiple_records_and_boundaries() {
        let seqs = vec![64, 65, 66, 67, 127];
        let mut stream = Vec::new();
        for &seq in &seqs {
            let entry = make_test_entry(seq);
            let xdr_bytes = entry.to_xdr(Limits::none()).unwrap();
            stream.extend(frame_record(&xdr_bytes, true));
        }

        let parsed = parse_ledger_header_stream(&stream, "test.xdr").unwrap();
        assert_eq!(parsed.len(), 5);

        // First boundary
        assert_eq!(parsed.first().unwrap().header.ledger_seq, 64);
        // Middle entry
        assert_eq!(parsed[2].header.ledger_seq, 66);
        // Last boundary
        assert_eq!(parsed.last().unwrap().header.ledger_seq, 127);
    }

    #[test]
    fn test_multi_fragment_record() {
        let entry = make_test_entry(200);
        let xdr_bytes = entry.to_xdr(Limits::none()).unwrap();
        assert!(xdr_bytes.len() > 4);

        let mid = xdr_bytes.len() / 2;
        let mut stream = Vec::new();
        stream.extend(frame_record(&xdr_bytes[..mid], false));
        stream.extend(frame_record(&xdr_bytes[mid..], true));

        let parsed = parse_ledger_header_stream(&stream, "test.xdr").unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].header.ledger_seq, 200);
    }

    #[test]
    fn test_ledger_not_found_in_stream() {
        let entry = make_test_entry(100);
        let xdr_bytes = entry.to_xdr(Limits::none()).unwrap();
        let framed = frame_record(&xdr_bytes, true);

        let parsed = parse_ledger_header_stream(&framed, "test.xdr").unwrap();
        let found = parsed.iter().find(|e| e.header.ledger_seq == 101);
        assert!(found.is_none());
    }

    #[test]
    fn test_truncated_frame_header() {
        let bytes = vec![0x80, 0x00, 0x00]; // 3 bytes instead of 4
        let err = parse_ledger_header_stream(&bytes, "test.xdr").unwrap_err();

        if let GratError::ArchiveError(ArchiveErrorKind::MalformedXdr { reason, .. }) = err {
            assert!(reason.contains("truncated record header"));
        } else {
            panic!("expected MalformedXdr error, got {:?}", err);
        }
    }

    #[test]
    fn test_truncated_frame_payload() {
        let payload = vec![1, 2, 3];
        // Frame header declares length 10, but only 3 payload bytes provided
        let mut framed = 10u32.to_be_bytes().to_vec();
        framed[0] |= 0x80; // mark as last
        framed.extend_from_slice(&payload);

        let err = parse_ledger_header_stream(&framed, "test.xdr").unwrap_err();
        if let GratError::ArchiveError(ArchiveErrorKind::MalformedXdr { reason, .. }) = err {
            assert!(reason.contains("truncated fragment payload"));
        } else {
            panic!("expected MalformedXdr error, got {:?}", err);
        }
    }

    #[test]
    fn test_unterminated_fragment_stream() {
        let payload = vec![1, 2, 3, 4];
        // Frame header declares length 4, but is_last = false
        let framed = frame_record(&payload, false);

        let err = parse_ledger_header_stream(&framed, "test.xdr").unwrap_err();
        if let GratError::ArchiveError(ArchiveErrorKind::MalformedXdr { reason, .. }) = err {
            assert!(reason.contains("unterminated record fragment stream"));
        } else {
            panic!("expected MalformedXdr error, got {:?}", err);
        }
    }

    #[test]
    fn test_invalid_xdr_payload() {
        let invalid_xdr = vec![0xFF; 20];
        let framed = frame_record(&invalid_xdr, true);

        let err = parse_ledger_header_stream(&framed, "test.xdr").unwrap_err();
        if let GratError::ArchiveError(ArchiveErrorKind::MalformedXdr { reason, .. }) = err {
            assert!(reason.contains("failed to decode LedgerHeaderHistoryEntry"));
        } else {
            panic!("expected MalformedXdr error, got {:?}", err);
        }
    }
}
