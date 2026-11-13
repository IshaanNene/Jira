//! What a notification email says. Every value that came from a person (names, issue
//! summaries) is escaped for HTML and stripped of line breaks in the subject.

use crate::domain::Kind;

#[derive(Debug, Clone)]
pub struct EmailInput<'a> {
    pub kind: Kind,
    pub actor_name: &'a str,
    pub issue_key: &'a str,
    pub issue_summary: &'a str,
    /// For status changes: the new status key.
    pub to_status: Option<&'a str>,
    /// Base URL of the web app, without a trailing slash.
    pub app_url: &'a str,
    /// Page that turns this kind of email off without signing in.
    pub unsubscribe_url: Option<&'a str>,
}

/// "comments", "status changes", … for "Stop emails about …".
pub fn kind_label(kind: Kind) -> &'static str {
    match kind {
        Kind::Assigned => "assignments",
        Kind::Mentioned => "mentions",
        Kind::Commented => "comments",
        Kind::StatusChanged => "status changes",
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendered {
    pub subject: String,
    pub text: String,
    pub html: String,
}

pub fn escape_html(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// One line, no control characters, bounded length.
fn one_line(value: &str, max: usize) -> String {
    let flat: String = value.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let flat = flat.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > max {
        format!("{}…", flat.chars().take(max - 1).collect::<String>())
    } else {
        flat
    }
}

/// `PAY-12` → `PAY`, only for keys of the expected shape (they are path segments).
fn project_key(issue_key: &str) -> Option<&str> {
    let (project, number) = issue_key.rsplit_once('-')?;
    let ok = !project.is_empty()
        && project.chars().all(|c| c.is_ascii_alphanumeric())
        && !number.is_empty()
        && number.chars().all(|c| c.is_ascii_digit());
    ok.then_some(project)
}

fn status_label(key: &str) -> String {
    let words = key.replace('_', " ");
    let mut chars = words.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

pub fn render(input: &EmailInput<'_>) -> Rendered {
    let who = if input.actor_name.trim().is_empty() { "Someone".to_owned() } else { one_line(input.actor_name, 80) };
    let summary = one_line(input.issue_summary, 150);
    let key = one_line(input.issue_key, 40);
    let action = match input.kind {
        Kind::Assigned => "assigned you".to_owned(),
        Kind::Mentioned => "mentioned you on".to_owned(),
        Kind::Commented => "commented on".to_owned(),
        Kind::StatusChanged => match input.to_status {
            Some(to) => format!("changed the status to {} on", status_label(&one_line(to, 40))),
            None => "changed the status of".to_owned(),
        },
    };
    let issue_url = project_key(&key).map(|p| format!("{}/projects/{p}/issues/{key}", input.app_url));
    let settings_url = format!("{}/settings/notifications", input.app_url);

    let subject = format!("[{key}] {summary}");
    let headline = format!("{who} {action} {key}");
    let mut text = format!("{headline}\n\n{key}: {summary}\n");
    if let Some(url) = &issue_url {
        text.push_str(&format!("\nView the issue: {url}\n"));
    }
    text.push_str(&format!(
        "\n--\nYou are receiving this because you are involved in this issue.\nChoose which emails you get: {settings_url}\n"
    ));
    if let Some(url) = input.unsubscribe_url {
        text.push_str(&format!("Stop emails about {}: {url}\n", kind_label(input.kind)));
    }
    let unsubscribe = input
        .unsubscribe_url
        .map(|u| {
            format!(r#" · <a href="{}" style="color:#0c66e4">Stop emails about {}</a>"#, escape_html(u), kind_label(input.kind))
        })
        .unwrap_or_default();

    let button = issue_url
        .as_ref()
        .map(|u| {
            format!(
                r#"<p style="margin:24px 0"><a href="{}" style="background:#0c66e4;color:#ffffff;padding:8px 14px;border-radius:3px;text-decoration:none;font-weight:600">View issue</a></p>"#,
                escape_html(u)
            )
        })
        .unwrap_or_default();
    let html = format!(
        r#"<!doctype html><html><body style="margin:0;padding:24px;background:#f7f8f9;font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,sans-serif;color:#172b4d">
<div style="max-width:560px;margin:0 auto;background:#ffffff;border:1px solid #dcdfe4;border-radius:6px;padding:24px">
<p style="margin:0 0 16px;font-size:14px">{headline}</p>
<p style="margin:0;font-size:16px;font-weight:600"><span style="color:#44546f">{key}</span> {summary}</p>
{button}
<hr style="border:none;border-top:1px solid #dcdfe4;margin:24px 0 12px">
<p style="margin:0;font-size:12px;color:#626f86">You are receiving this because you are involved in this issue. <a href="{settings}" style="color:#0c66e4">Choose which emails you get</a>{unsubscribe}.</p>
</div></body></html>"#,
        headline = escape_html(&headline),
        key = escape_html(&key),
        summary = escape_html(&summary),
        settings = escape_html(&settings_url),
    );
    Rendered { subject, text, html }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(kind: Kind) -> EmailInput<'static> {
        EmailInput {
            kind,
            actor_name: "Maya <script>",
            issue_key: "PAY-12",
            issue_summary: "Refunds\r\nBcc: victim@example.com <b>fail</b>",
            to_status: Some("in_review"),
            app_url: "https://app.example.com",
            unsubscribe_url: Some("https://app.example.com/unsubscribe?token=abc.def"),
        }
    }

    #[test]
    fn content_from_people_cannot_inject_headers_or_markup() {
        let r = render(&input(Kind::Assigned));
        assert_eq!(r.subject, "[PAY-12] Refunds Bcc: victim@example.com <b>fail</b>");
        assert!(!r.subject.contains('\n') && !r.subject.contains('\r'));
        assert!(r.html.contains("Maya &lt;script&gt; assigned you PAY-12"));
        assert!(!r.html.contains("<script>") && !r.html.contains("<b>fail"));
        assert!(r.text.contains("View the issue: https://app.example.com/projects/PAY/issues/PAY-12"));
        assert!(r.text.contains("Stop emails about assignments: https://app.example.com/unsubscribe?token=abc.def"));
        assert!(r.html.contains(r#"href="https://app.example.com/unsubscribe?token=abc.def""#));
    }

    #[test]
    fn says_what_happened() {
        assert!(render(&input(Kind::StatusChanged)).text.starts_with("Maya <script> changed the status to In review on PAY-12"));
        assert!(render(&input(Kind::Mentioned)).text.starts_with("Maya <script> mentioned you on PAY-12"));
        let odd = EmailInput { issue_key: "../../x", actor_name: "", ..input(Kind::Commented) };
        let r = render(&odd);
        assert!(r.text.starts_with("Someone commented on"));
        assert!(!r.text.contains("View the issue"), "no link for a key of the wrong shape");
        let long = "x".repeat(400);
        assert!(render(&EmailInput { issue_summary: &long, ..input(Kind::Assigned) }).subject.chars().count() < 170);
    }
}
