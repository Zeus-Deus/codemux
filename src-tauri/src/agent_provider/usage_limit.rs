//! Shared interpretation of provider limit errors. Timing must come from
//! structured provider data; human-readable reset clocks are never guessed.

use serde_json::Value;

use super::{ProviderKind, ProviderRuntimeEvent, ThreadId};

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn error_objects(payload: &Value) -> impl Iterator<Item = &Value> {
    std::iter::once(payload)
        .chain(
            [
                "data",
                "error",
                "_meta",
                "rate_limit_info",
                "rateLimitInfo",
                "rateLimit",
                "rate_limit",
                "agentResult",
            ]
            .into_iter()
            .filter_map(|key| payload.get(key)),
        )
        .chain(payload.pointer("/_meta/agentResult"))
}

pub fn is_usage_limit_error(message: &str, payload: &Value) -> bool {
    let lower = message.to_ascii_lowercase();
    if lower.contains("billing")
        || lower.contains("authentication")
        || lower.contains("invalid api key")
        || error_objects(payload).any(|object| {
            ["name", "code", "type", "error"].iter().any(|key| {
                object.get(key).and_then(Value::as_str).is_some_and(|kind| {
                    let kind = kind.to_ascii_lowercase();
                    kind.contains("billing")
                        || kind.contains("authentication")
                        || kind.contains("unauthorized")
                })
            })
        })
    {
        return false;
    }
    let text_limit = |text: &str| {
        let text = text.to_ascii_lowercase();
        text.contains("usage limit")
            || text.contains("rate limit")
            || matches!(
                text.as_str(),
                "rate_limit" | "ratelimiterror" | "usagelimitexceeded"
            )
            || text.contains("too many requests")
    };
    text_limit(message)
        || error_objects(payload).any(|object| {
            [
                "httpStatusCode",
                "http_status",
                "httpStatus",
                "statusCode",
                "status",
            ]
            .iter()
            .any(|key| object.get(key).and_then(Value::as_u64) == Some(429))
                || ["name", "code", "type", "error", "stopReason", "message"]
                    .iter()
                    .any(|key| {
                        object
                            .get(key)
                            .and_then(Value::as_str)
                            .is_some_and(text_limit)
                    })
        })
}

fn epoch_ms(value: &Value) -> Option<i64> {
    let number = value
        .as_f64()
        .or_else(|| value.as_str()?.parse::<f64>().ok());
    if let Some(number) = number.filter(|n| n.is_finite() && *n > 0.0) {
        let ms = if number < 100_000_000_000.0 {
            number * 1000.0
        } else {
            number
        };
        return (ms < i64::MAX as f64).then_some(ms as i64);
    }
    chrono::DateTime::parse_from_rfc3339(value.as_str()?)
        .ok()
        .map(|at| at.timestamp_millis())
}

fn retry_after_ms(value: &Value, now: i64) -> Option<i64> {
    let seconds = value
        .as_f64()
        .or_else(|| value.as_str()?.parse::<f64>().ok());
    if let Some(seconds) = seconds.filter(|n| n.is_finite() && *n >= 0.0) {
        let delay = seconds * 1000.0;
        return (delay < i64::MAX as f64)
            .then(|| now.checked_add(delay as i64))
            .flatten();
    }
    chrono::DateTime::parse_from_rfc2822(value.as_str()?)
        .ok()
        .map(|at| at.timestamp_millis())
}

/// Absolute reset epochs or a Retry-After supplied with a terminal error.
/// Retry-After is relative to receipt; passing the clock makes it testable.
pub fn reported_reset_ms(payload: &Value, now: i64) -> Option<i64> {
    error_objects(payload)
        .filter_map(|object| {
            let absolute = ["resets_at_ms", "reset_at_ms"]
                .iter()
                .filter_map(|key| object.get(key)?.as_i64())
                .filter(|at| *at > 0)
                .chain(
                    ["resetsAt", "resetAt", "reset_at", "resets_at"]
                        .iter()
                        .filter_map(|key| epoch_ms(object.get(key)?)),
                )
                .max();
            let retry = ["retryAfter", "retry_after", "retryAfterSeconds"]
                .iter()
                .filter_map(|key| retry_after_ms(object.get(key)?, now))
                .max();
            let headers = object
                .get("responseHeaders")
                .or_else(|| object.get("headers"))
                .and_then(Value::as_object)
                .and_then(|headers| {
                    headers
                        .iter()
                        .filter(|(key, _)| key.eq_ignore_ascii_case("retry-after"))
                        .filter_map(|(_, value)| retry_after_ms(value, now))
                        .max()
                });
            [absolute, retry, headers].into_iter().flatten().max()
        })
        .max()
}

pub fn usage_limit_notice(
    thread: &ThreadId,
    provider: ProviderKind,
    payload: &Value,
    message: &str,
) -> Option<ProviderRuntimeEvent> {
    is_usage_limit_error(message, payload).then(|| ProviderRuntimeEvent::UsageLimitReached {
        thread_id: thread.clone(),
        provider,
        resets_at_ms: reported_reset_ms(payload, now_ms()),
        auto_resume_at_ms: None,
        auto_resume_blocked_reason: None,
        window: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn usage_limit_timing_uses_reported_epochs_and_retry_headers() {
        let now = 1_800_000_000_000;
        assert_eq!(
            reported_reset_ms(&json!({"resetsAt": 1_800_000_100}), now),
            Some(now + 100_000)
        );
        assert_eq!(
            reported_reset_ms(&json!({"_meta": {"reset_at_ms": now + 200_000}}), now),
            Some(now + 200_000)
        );
        assert_eq!(
            reported_reset_ms(
                &json!({"data": {"responseHeaders": {"Retry-After": "120"}}}),
                now
            ),
            Some(now + 120_000)
        );
        assert_eq!(
            reported_reset_ms(&json!({"retryAfter": 1.5}), now),
            Some(now + 1500)
        );
        assert_eq!(
            reported_reset_ms(&json!({"resetAt":"2027-01-15T08:00:00Z"}), now),
            Some(now)
        );
        assert_eq!(
            reported_reset_ms(
                &json!({"headers":{"retry-after":"Fri, 15 Jan 2027 08:00:00 GMT"}}),
                now
            ),
            Some(now)
        );
        assert_eq!(
            reported_reset_ms(&json!({"_meta":{"agentResult":{"reset_at_ms":now}}}), now),
            Some(now)
        );
        assert_eq!(
            reported_reset_ms(&json!({"message": "resets 1:40am (Europe/Brussels)"}), now),
            None
        );
        assert_eq!(reported_reset_ms(&json!({"retryAfter": -1}), now), None);
        assert_eq!(reported_reset_ms(&json!({"retryAfter": "inf"}), now), None);
        assert_eq!(reported_reset_ms(&json!({"retryAfter": 1e30}), now), None);
    }

    #[test]
    fn usage_limit_errors_exclude_billing_and_authentication() {
        assert!(is_usage_limit_error("", &json!({"statusCode":429})));
        assert!(is_usage_limit_error(
            "",
            &json!({"stopReason":"rate_limit"})
        ));
        assert!(is_usage_limit_error(
            "You've hit your usage limit",
            &Value::Null
        ));
        for message in [
            "Billing error",
            "Authentication failed",
            "Invalid API key",
            "Account credits exhausted",
            "Connection lost",
        ] {
            assert!(!is_usage_limit_error(message, &Value::Null));
        }
        assert!(!is_usage_limit_error(
            "Billing error",
            &json!({"statusCode":429})
        ));
        assert!(!is_usage_limit_error(
            "usage limit",
            &json!({"error":"billing_error", "retryAfter":60})
        ));
    }
}
