//! Environment-based configuration helpers. Services fail fast at startup when a
//! required variable is missing instead of discovering it on the first request.

use std::str::FromStr;

use anyhow::{anyhow, Context};

pub fn required(name: &str) -> anyhow::Result<String> {
    match std::env::var(name) {
        Ok(v) if !v.trim().is_empty() => Ok(v),
        _ => Err(anyhow!("required environment variable {name} is not set")),
    }
}

pub fn optional(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

pub fn or(name: &str, default: &str) -> String {
    optional(name).unwrap_or_else(|| default.to_owned())
}

pub fn parse_or<T>(name: &str, default: T) -> anyhow::Result<T>
where
    T: FromStr,
    T::Err: std::error::Error + Send + Sync + 'static,
{
    match optional(name) {
        Some(v) => v.parse::<T>().with_context(|| format!("invalid value for {name}")),
        None => Ok(default),
    }
}

/// Comma-separated list, trimmed, empty entries removed.
pub fn list(name: &str) -> Vec<String> {
    optional(name).map(|v| v.split(',').map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()).collect()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_required_is_an_error() {
        assert!(required("PLATFORM_COMMON_TEST_DOES_NOT_EXIST").is_err());
    }

    #[test]
    fn parse_or_uses_default_when_unset() {
        assert_eq!(parse_or::<u16>("PLATFORM_COMMON_TEST_UNSET_PORT", 8080).ok(), Some(8080));
    }
}
