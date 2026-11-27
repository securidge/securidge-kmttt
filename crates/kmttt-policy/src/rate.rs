//! 호출자별 미끄러지는 창(sliding window) 호출 수 제한.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

#[derive(Default)]
pub struct RateLimiter {
    hits: HashMap<(String, String), VecDeque<Instant>>,
}

impl RateLimiter {
    /// 이번 호출을 허용하면 true(그리고 센다).
    pub fn check(&mut self, caller: &str, bucket: &str, max: u32, window_secs: u64) -> bool {
        let now = Instant::now();
        let q = self
            .hits
            .entry((caller.to_string(), bucket.to_string()))
            .or_default();
        let window = Duration::from_secs(window_secs.max(1));
        while q.front().is_some_and(|t| now.duration_since(*t) > window) {
            q.pop_front();
        }
        if q.len() as u32 >= max {
            return false;
        }
        q.push_back(now);
        true
    }
}

/// "10/min", "5/sec", "100/hour" → (횟수, 초)
pub fn parse_rate(s: &str) -> Option<(u32, u64)> {
    let (n, unit) = s.split_once('/')?;
    let n: u32 = n.trim().parse().ok()?;
    let secs = match unit.trim() {
        "s" | "sec" | "second" => 1,
        "m" | "min" | "minute" => 60,
        "h" | "hour" => 3600,
        _ => return None,
    };
    Some((n, secs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse() {
        assert_eq!(parse_rate("10/min"), Some((10, 60)));
        assert_eq!(parse_rate("5 / sec"), Some((5, 1)));
        assert_eq!(parse_rate("x/min"), None);
        assert_eq!(parse_rate("10/day"), None);
    }

    #[test]
    fn limits_per_caller_and_bucket() {
        let mut r = RateLimiter::default();
        assert!(r.check("a", "b", 2, 60));
        assert!(r.check("a", "b", 2, 60));
        assert!(!r.check("a", "b", 2, 60));
        assert!(r.check("a", "other", 2, 60));
        assert!(r.check("z", "b", 2, 60));
    }
}
