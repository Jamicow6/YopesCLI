use super::*;

pub(super) fn estimated_output_tokens(session: &Session) -> u64 {
    match session.messages.last() {
        Some(message) if message.role == Role::Assistant => {
            (message.content.chars().count() + message.reasoning.chars().count()) as u64 / 4
        }
        _ => 0,
    }
}

pub fn format_tokens(tokens: u64) -> String {
    if tokens < 1000 {
        tokens.to_string()
    } else {
        format!("{:.1}k", tokens as f64 / 1000.0)
    }
}

/// Dollars per million tokens, `(input, output)`, for the common models when
/// the provider catalog has no price. `None` means the price is unknown: the
/// session counter shows a dash instead of inventing a bill.
pub(super) fn known_model_price(model: &str) -> Option<(f64, f64)> {
    let model = model.to_ascii_lowercase();
    let table: [(&str, f64, f64); 8] = [
        ("gpt-4o-mini", 0.15, 0.60),
        ("gpt-4o", 2.50, 10.0),
        ("gpt-4.1", 2.00, 8.00),
        ("o3", 2.00, 8.00),
        ("o4-mini", 1.10, 4.40),
        ("claude-3-5-sonnet", 3.00, 15.0),
        ("claude-3-5-haiku", 0.80, 4.00),
        ("deepseek", 0.27, 1.10),
    ];
    // Longest match wins so `gpt-4o-mini` does not bill as `gpt-4o`.
    let mut best: Option<(&str, f64, f64)> = None;
    for (name, input, output) in table {
        if model.contains(name) && best.is_none_or(|(seen, _, _)| name.len() > seen.len()) {
            best = Some((name, input, output));
        }
    }
    best.map(|(_, input, output)| (input, output))
}

/// Session spend as `$0.0000`, priced from the active model. A model with no
/// known price shows `—`; a free one shows `free` rather than a fake bill.
pub fn format_cost(
    prompt_tokens: u64,
    completion_tokens: u64,
    price: Option<(f64, f64)>,
) -> String {
    let Some((input, output)) = price else {
        return "—".to_owned();
    };
    if input == 0.0 && output == 0.0 {
        return "free".to_owned();
    }
    let dollars = (prompt_tokens as f64 * input + completion_tokens as f64 * output) / 1_000_000.0;
    // Small sessions cost fractions of a cent, so keep four decimals until it
    // is worth showing cents.
    if dollars < 1.0 {
        format!("${dollars:.4}")
    } else {
        format!("${dollars:.2}")
    }
}

pub fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    }
}

#[cfg(test)]
mod tests {
    use super::{format_bytes, format_tokens};

    #[test]
    fn formats_keep_tokens_and_bytes_compact() {
        assert_eq!(format_tokens(999), "999");
        assert_eq!(format_tokens(1200), "1.2k");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(2150), "2.1 KB");
    }
}
