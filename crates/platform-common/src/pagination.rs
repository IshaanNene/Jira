//! Keyset pagination primitives. Cursors are opaque to clients: base64url(JSON).

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

pub const DEFAULT_LIMIT: u32 = 50;
pub const MAX_LIMIT: u32 = 200;

#[derive(Debug, Clone, Deserialize)]
pub struct PageParams {
    pub limit: Option<u32>,
    pub cursor: Option<String>,
}

impl PageParams {
    pub fn limit(&self) -> i64 {
        i64::from(self.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT))
    }

    /// `None` when no cursor was sent; `Err` when one was sent but is not decodable.
    pub fn decode_cursor<C: DeserializeOwned>(&self) -> Result<Option<C>, InvalidCursor> {
        match &self.cursor {
            None => Ok(None),
            Some(raw) => decode_cursor(raw).map(Some),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct InvalidCursor;

#[derive(Debug, Clone, Serialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
}

impl<T> Page<T> {
    /// Build a page from `limit + 1` fetched rows: the extra row only signals that more exist.
    pub fn from_overfetch<C: Serialize>(mut rows: Vec<T>, limit: i64, cursor_of: impl Fn(&T) -> C) -> Self {
        let limit = usize::try_from(limit).unwrap_or(0);
        let has_more = rows.len() > limit;
        rows.truncate(limit);
        let next_cursor = if has_more { rows.last().map(|r| encode_cursor(&cursor_of(r))) } else { None };
        Page { items: rows, next_cursor }
    }
}

pub fn encode_cursor<C: Serialize>(cursor: &C) -> String {
    // Serializing a plain data struct to JSON cannot fail; fall back to an empty cursor defensively.
    URL_SAFE_NO_PAD.encode(serde_json::to_vec(cursor).unwrap_or_default())
}

pub fn decode_cursor<C: DeserializeOwned>(raw: &str) -> Result<C, InvalidCursor> {
    let bytes = URL_SAFE_NO_PAD.decode(raw).map_err(|_| InvalidCursor)?;
    serde_json::from_slice(&bytes).map_err(|_| InvalidCursor)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_round_trip() {
        let c = (42_i64, "abc".to_owned());
        let decoded: (i64, String) = decode_cursor(&encode_cursor(&c)).unwrap();
        assert_eq!(decoded, c);
    }

    #[test]
    fn garbage_cursor_is_rejected() {
        assert_eq!(decode_cursor::<(i64, String)>("not base64 !!"), Err(InvalidCursor));
    }

    #[test]
    fn overfetch_sets_next_cursor_only_when_more_rows_exist() {
        let page = Page::from_overfetch(vec![1, 2, 3], 2, |n| *n);
        assert_eq!(page.items, vec![1, 2]);
        assert!(page.next_cursor.is_some());
        let last = Page::from_overfetch(vec![1, 2], 2, |n| *n);
        assert!(last.next_cursor.is_none());
    }

    #[test]
    fn limit_is_clamped() {
        let p = PageParams { limit: Some(10_000), cursor: None };
        assert_eq!(p.limit(), i64::from(MAX_LIMIT));
    }
}
