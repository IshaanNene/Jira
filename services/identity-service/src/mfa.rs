//! Two-step verification: time-based one-time passwords (RFC 6238, the format every
//! authenticator app reads) and single-use recovery codes. Pure functions; storage is in
//! `http::mfa`.

use rand::{rngs::OsRng, Rng, RngCore};
use ring::hmac;
use subtle::ConstantTimeEq;

/// Shown in the authenticator app next to the account.
pub const ISSUER: &str = "Meridian";
/// Seconds per code.
pub const PERIOD: i64 = 30;
const DIGITS: u32 = 6;
/// Codes from one step either side are accepted, for clocks that drift.
const SKEW: i64 = 1;
pub const RECOVERY_CODE_COUNT: usize = 10;
/// Crockford's base32 alphabet: no I, L, O or U, so codes are easy to read and type.
const RECOVERY_ALPHABET: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const RECOVERY_LEN: usize = 12;

/// 160 random bits, the size RFC 4226 recommends for HMAC-SHA1.
pub fn new_secret() -> [u8; 20] {
    let mut secret = [0_u8; 20];
    OsRng.fill_bytes(&mut secret);
    secret
}

/// RFC 4648 base32 without padding, as authenticator apps expect.
pub fn base32(bytes: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut out = String::with_capacity(bytes.len().div_ceil(5) * 8);
    let (mut buffer, mut bits) = (0_u32, 0_u32);
    for &b in bytes {
        buffer = (buffer << 8) | u32::from(b);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(ALPHABET[((buffer >> bits) & 31) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(ALPHABET[((buffer << (5 - bits)) & 31) as usize] as char);
    }
    out
}

/// The time step a Unix timestamp falls in.
pub fn step_at(unix_seconds: i64) -> i64 {
    unix_seconds.div_euclid(PERIOD)
}

/// The code for one time step (RFC 4226 HOTP over the step counter, HMAC-SHA1).
pub fn code_at(secret: &[u8], step: i64) -> String {
    let key = hmac::Key::new(hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, secret);
    let tag = hmac::sign(&key, &step.to_be_bytes());
    let mac = tag.as_ref();
    let offset = usize::from(mac[mac.len() - 1] & 0x0f);
    let binary = u32::from_be_bytes([mac[offset] & 0x7f, mac[offset + 1], mac[offset + 2], mac[offset + 3]]);
    format!("{:0width$}", binary % 10_u32.pow(DIGITS), width = DIGITS as usize)
}

/// `Some(digits)` when the input looks like an authenticator code ("123 456" is fine).
pub fn as_totp(input: &str) -> Option<String> {
    let digits: String = input.chars().filter(|c| !c.is_whitespace()).collect();
    (digits.len() == DIGITS as usize && digits.bytes().all(|b| b.is_ascii_digit())).then_some(digits)
}

/// Checks a code against the steps around `now_step`. Returns the matching step, which
/// must be later than `last_used_step`: a code that has been used once never works again.
pub fn verify(secret: &[u8], code: &str, now_step: i64, last_used_step: i64) -> Option<i64> {
    let code = as_totp(code)?;
    let mut matched = None;
    // Every candidate is compared, in constant time, whatever matches first.
    for step in (now_step - SKEW)..=(now_step + SKEW) {
        let ok: bool = code_at(secret, step).as_bytes().ct_eq(code.as_bytes()).into();
        if ok && step > last_used_step && matched.is_none() {
            matched = Some(step);
        }
    }
    matched
}

fn percent_encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// The `otpauth://` link an authenticator app imports (Key Uri Format).
pub fn otpauth_uri(account_email: &str, secret_base32: &str) -> String {
    format!(
        "otpauth://totp/{issuer}:{label}?secret={secret_base32}&issuer={issuer}&algorithm=SHA1&digits={DIGITS}&period={PERIOD}",
        issuer = percent_encode(ISSUER),
        label = percent_encode(account_email),
    )
}

/// The link as a QR code, as a standalone SVG document (no scripts, no external refs).
pub fn qr_svg(uri: &str) -> anyhow::Result<String> {
    use qrcode::{render::svg, EcLevel, QrCode};
    let code = QrCode::with_error_correction_level(uri.as_bytes(), EcLevel::M)?;
    Ok(code
        .render::<svg::Color<'_>>()
        .min_dimensions(184, 184)
        .dark_color(svg::Color("#111111"))
        .light_color(svg::Color("#ffffff"))
        .quiet_zone(true)
        .build())
}

/// Ten fresh recovery codes, 60 bits each, shown as `XXXX-XXXX-XXXX`.
pub fn new_recovery_codes() -> Vec<String> {
    (0..RECOVERY_CODE_COUNT)
        .map(|_| {
            let raw: String = (0..RECOVERY_LEN).map(|_| RECOVERY_ALPHABET[OsRng.gen_range(0..32)] as char).collect();
            format!("{}-{}-{}", &raw[0..4], &raw[4..8], &raw[8..12])
        })
        .collect()
}

/// The canonical form of a typed recovery code (case, dashes and spaces ignored, and the
/// look-alike letters Crockford maps: O→0, I and L→1), or `None` if it cannot be one.
pub fn normalize_recovery_code(input: &str) -> Option<String> {
    let code: String = input
        .chars()
        .filter(|c| !matches!(c, '-' | ' '))
        .map(|c| match c.to_ascii_uppercase() {
            'O' => '0',
            'I' | 'L' => '1',
            other => other,
        })
        .collect();
    (code.len() == RECOVERY_LEN && code.bytes().all(|b| RECOVERY_ALPHABET.contains(&b))).then_some(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RFC_SECRET: &[u8] = b"12345678901234567890";

    #[test]
    fn matches_the_rfc_6238_test_vectors() {
        // RFC 6238 appendix B, SHA-1, truncated to six digits.
        for (time, expected) in [(59, "287082"), (1_111_111_109, "081804"), (1_234_567_890, "005924"), (2_000_000_000, "279037")]
        {
            assert_eq!(code_at(RFC_SECRET, step_at(time)), expected, "t = {time}");
        }
    }

    #[test]
    fn base32_matches_rfc_4648() {
        assert_eq!(base32(b""), "");
        assert_eq!(base32(b"f"), "MY");
        assert_eq!(base32(b"foobar"), "MZXW6YTBOI");
        assert_eq!(base32(RFC_SECRET), "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ");
        assert_eq!(base32(&new_secret()).len(), 32);
    }

    #[test]
    fn accepts_neighbouring_steps_once_and_never_replays() {
        let now = step_at(1_234_567_890);
        let current = code_at(RFC_SECRET, now);
        assert_eq!(verify(RFC_SECRET, &current, now, 0), Some(now));
        assert_eq!(verify(RFC_SECRET, &format!("{} {}", &current[..3], &current[3..]), now, 0), Some(now));
        // Already used: refused, and so is anything older.
        assert_eq!(verify(RFC_SECRET, &current, now, now), None);
        assert_eq!(verify(RFC_SECRET, &code_at(RFC_SECRET, now - 1), now, now), None);
        // A slow clock's code still works; one two steps away does not.
        assert_eq!(verify(RFC_SECRET, &code_at(RFC_SECRET, now + 1), now, 0), Some(now + 1));
        assert_eq!(verify(RFC_SECRET, &code_at(RFC_SECRET, now - 2), now, 0), None);
        assert_eq!(verify(RFC_SECRET, "12345", now, 0), None);
        assert_eq!(verify(RFC_SECRET, "abcdef", now, 0), None);
    }

    #[test]
    fn otpauth_links_are_encoded() {
        let uri = otpauth_uri("ada+test@example.com", "ABC");
        assert_eq!(
            uri,
            "otpauth://totp/Meridian:ada%2Btest%40example.com?secret=ABC&issuer=Meridian&algorithm=SHA1&digits=6&period=30"
        );
        let svg = qr_svg(&uri).unwrap();
        assert!(svg.starts_with("<?xml") && svg.contains("<svg") && !svg.contains("<script"));
    }

    #[test]
    fn recovery_codes_are_distinct_and_forgiving_to_type() {
        let codes = new_recovery_codes();
        assert_eq!(codes.len(), 10);
        let mut unique = codes.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), 10);
        for code in &codes {
            assert_eq!(code.len(), 14);
            assert_eq!(normalize_recovery_code(code).unwrap().len(), 12);
            assert_eq!(normalize_recovery_code(&code.to_lowercase().replace('-', " ")), normalize_recovery_code(code));
        }
        assert_eq!(normalize_recovery_code("abcd-efgh-o1l0").as_deref(), Some("ABCDEFGH0110"));
        assert_eq!(normalize_recovery_code("ABCD-EFGH-JKU0"), None); // U is not in the alphabet
        assert_eq!(normalize_recovery_code("123456"), None);
    }
}
