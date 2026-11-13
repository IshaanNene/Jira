//! Validation of rich-text documents (the editor's JSON format) before storage.
//!
//! Documents are stored and rendered as structured data, never as HTML. This validator
//! is the server-side half of XSS protection: only known node and mark types pass, link
//! targets must use a safe scheme, and size and depth are bounded.

use error_handling::{ApiError, ApiResult};
use serde_json::Value;
use uuid::Uuid;

const MAX_BYTES: usize = 100_000;
const MAX_DEPTH: usize = 20;
const NODES: &[&str] = &[
    "doc",
    "paragraph",
    "text",
    "heading",
    "bulletList",
    "orderedList",
    "listItem",
    "codeBlock",
    "blockquote",
    "hardBreak",
    "horizontalRule",
    "mention",
];
const MARKS: &[&str] = &["bold", "italic", "strike", "code", "link"];

fn invalid(field: &str, message: &str) -> ApiError {
    ApiError::invalid(field, "invalid_document", message)
}

/// Validate `doc` and return the user ids it mentions.
pub fn validate(field: &str, doc: &Value) -> ApiResult<Vec<Uuid>> {
    let size = serde_json::to_vec(doc).map(|v| v.len()).unwrap_or(usize::MAX);
    if size > MAX_BYTES {
        return Err(invalid(field, "The text is too long."));
    }
    if doc.get("type").and_then(Value::as_str) != Some("doc") {
        return Err(invalid(field, "The text is not a valid document."));
    }
    let mut mentions = Vec::new();
    node(field, doc, 0, &mut mentions)?;
    mentions.sort_unstable();
    mentions.dedup();
    Ok(mentions)
}

fn node(field: &str, value: &Value, depth: usize, mentions: &mut Vec<Uuid>) -> ApiResult<()> {
    if depth > MAX_DEPTH {
        return Err(invalid(field, "The text is nested too deeply."));
    }
    let object = value.as_object().ok_or_else(|| invalid(field, "The text is not a valid document."))?;
    let kind = object.get("type").and_then(Value::as_str).unwrap_or("");
    if !NODES.contains(&kind) {
        return Err(invalid(field, "The text contains unsupported formatting."));
    }
    // Reject anything this validator does not understand rather than storing it blindly.
    if object.keys().any(|k| !matches!(k.as_str(), "type" | "content" | "text" | "marks" | "attrs")) {
        return Err(invalid(field, "The text contains unsupported formatting."));
    }
    if let Some(text) = object.get("text") {
        if kind != "text" || !text.is_string() {
            return Err(invalid(field, "The text is not a valid document."));
        }
    }
    if kind == "mention" {
        let id = object.get("attrs").and_then(|a| a.get("id")).and_then(Value::as_str).and_then(|s| Uuid::parse_str(s).ok());
        mentions.push(id.ok_or_else(|| invalid(field, "A mention does not refer to a person."))?);
    }
    if let Some(marks) = object.get("marks") {
        for mark in marks.as_array().ok_or_else(|| invalid(field, "The text is not a valid document."))? {
            let mark_type = mark.get("type").and_then(Value::as_str).unwrap_or("");
            if !MARKS.contains(&mark_type) {
                return Err(invalid(field, "The text contains unsupported formatting."));
            }
            if mark_type == "link" {
                let href = mark.get("attrs").and_then(|a| a.get("href")).and_then(Value::as_str).unwrap_or("");
                if !safe_href(href) {
                    return Err(invalid(field, "Links must start with http://, https:// or mailto:."));
                }
            }
        }
    }
    if let Some(content) = object.get("content") {
        for child in content.as_array().ok_or_else(|| invalid(field, "The text is not a valid document."))? {
            node(field, child, depth + 1, mentions)?;
        }
    }
    Ok(())
}

fn safe_href(href: &str) -> bool {
    let lower = href.trim_start().to_ascii_lowercase();
    href.len() <= 2000
        && !href.chars().any(char::is_control)
        && (lower.starts_with("https://") || lower.starts_with("http://") || lower.starts_with("mailto:"))
}

/// The visible text of a document, blocks separated by newlines (for search). Mentions
/// contribute their label; anything else without text contributes nothing.
pub fn plain_text(doc: &Value) -> String {
    fn walk(value: &Value, out: &mut String) {
        if let Some(text) = value.get("text").and_then(Value::as_str) {
            out.push_str(text);
        }
        if value.get("type").and_then(Value::as_str) == Some("mention") {
            if let Some(label) = value.pointer("/attrs/label").and_then(Value::as_str) {
                out.push('@');
                out.push_str(label);
            }
        }
        if let Some(children) = value.get("content").and_then(Value::as_array) {
            for child in children {
                walk(child, out);
            }
            let block = matches!(
                value.get("type").and_then(Value::as_str),
                Some("paragraph" | "heading" | "listItem" | "codeBlock" | "blockquote")
            );
            if block && !out.ends_with('\n') {
                out.push('\n');
            }
        }
    }
    let mut out = String::new();
    walk(doc, &mut out);
    out.trim().to_owned()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn doc(content: Value) -> Value {
        json!({ "type": "doc", "content": [content] })
    }

    #[test]
    fn accepts_ordinary_documents_and_collects_mentions() {
        let user = Uuid::now_v7();
        let d = json!({ "type": "doc", "content": [
            { "type": "heading", "attrs": { "level": 2 }, "content": [{ "type": "text", "text": "Plan" }] },
            { "type": "paragraph", "content": [
                { "type": "text", "text": "See ", "marks": [{ "type": "bold" }] },
                { "type": "text", "text": "docs", "marks": [{ "type": "link", "attrs": { "href": "https://example.com/a" } }] },
                { "type": "mention", "attrs": { "id": user.to_string(), "label": "Ada" } },
                { "type": "mention", "attrs": { "id": user.to_string(), "label": "Ada" } }
            ]},
            { "type": "bulletList", "content": [{ "type": "listItem", "content": [{ "type": "paragraph" }] }] }
        ]});
        assert_eq!(validate("description", &d).unwrap(), vec![user]);
    }

    #[test]
    fn rejects_script_vectors() {
        for href in [
            "javascript:alert(1)",
            " JaVaScRiPt:alert(1)",
            "data:text/html,<script>",
            "vbscript:x",
            "//evil.example",
            "java\nscript:alert(1)",
        ] {
            let d = doc(
                json!({ "type": "paragraph", "content": [{ "type": "text", "text": "x", "marks": [{ "type": "link", "attrs": { "href": href } }] }] }),
            );
            assert!(validate("d", &d).is_err(), "{href}");
        }
        assert!(validate("d", &doc(json!({ "type": "script", "content": [] }))).is_err());
        assert!(validate("d", &doc(json!({ "type": "paragraph", "html": "<img onerror=alert(1)>" }))).is_err());
        assert!(validate(
            "d",
            &doc(json!({ "type": "paragraph", "content": [{ "type": "text", "text": "x", "marks": [{ "type": "onclick" }] }] }))
        )
        .is_err());
        assert!(validate("d", &doc(json!({ "type": "mention", "attrs": { "id": "not-a-uuid" } }))).is_err());
    }

    #[test]
    fn rejects_malformed_oversized_and_deep_documents() {
        assert!(validate("d", &json!("just a string")).is_err());
        assert!(validate("d", &json!({ "type": "paragraph" })).is_err());
        assert!(validate("d", &doc(json!({ "type": "paragraph", "content": "nope" }))).is_err());
        let big = doc(json!({ "type": "paragraph", "content": [{ "type": "text", "text": "x".repeat(MAX_BYTES) }] }));
        assert!(validate("d", &big).is_err());
        let mut deep = json!({ "type": "paragraph" });
        for _ in 0..30 {
            deep = json!({ "type": "blockquote", "content": [deep] });
        }
        assert!(validate("d", &doc(deep)).is_err());
    }

    #[test]
    fn plain_text_keeps_words_and_block_breaks() {
        let doc = serde_json::json!({"type":"doc","content":[
            {"type":"paragraph","content":[{"type":"text","text":"Card "},{"type":"text","text":"declined","marks":[{"type":"bold"}]}]},
            {"type":"paragraph","content":[{"type":"mention","attrs":{"id":"x","label":"Maya"}},{"type":"text","text":" please look"}]}
        ]});
        assert_eq!(super::plain_text(&doc), "Card declined\n@Maya please look");
        assert_eq!(super::plain_text(&serde_json::json!({"type":"doc"})), "");
    }
}
