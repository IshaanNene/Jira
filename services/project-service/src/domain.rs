//! Project rules that need no I/O.

use error_handling::{ApiError, ApiResult};

/// Project keys prefix every issue key: 2–10 characters, uppercase letter first, then
/// uppercase letters or digits.
pub fn validate_key(key: &str) -> ApiResult<()> {
    let bytes = key.as_bytes();
    let ok = (2..=10).contains(&bytes.len())
        && bytes[0].is_ascii_uppercase()
        && bytes.iter().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit());
    if ok {
        Ok(())
    } else {
        Err(ApiError::invalid("key", "invalid_key", "Use 2–10 uppercase letters or digits, starting with a letter."))
    }
}

pub fn validate_type(project_type: &str) -> ApiResult<()> {
    if matches!(project_type, "scrum" | "kanban" | "general") {
        Ok(())
    } else {
        Err(ApiError::invalid("project_type", "unknown_type", "Type must be scrum, kanban or general."))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys() {
        for good in ["PAY", "A1", "PLATFORM10"] {
            assert!(validate_key(good).is_ok(), "{good}");
        }
        for bad in ["", "A", "pay", "1AB", "PAY-1", "TOOLONGKEY11", "PÄY", "PAY 1"] {
            assert!(validate_key(bad).is_err(), "{bad}");
        }
    }
}
