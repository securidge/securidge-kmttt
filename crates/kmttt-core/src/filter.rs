//! 이벤트 버스의 필터 체인.
//!
//! 서버 안에서 만들어지는 모든 이벤트(pane 출력, API 호출, 정책 판정, 상태 변화 …)는
//! `FilterChain::process`를 거친 뒤에만 기록·구독자·플러그인으로 나간다.
//! 순서는 마스킹(F3) → 기록(F2)이며, 정책(F1)은 API 게이트웨이에서 호출 단위로 판정한다.

use serde_json::Value;

/// 버스를 지나는 이벤트 한 건.
#[derive(Debug, Clone, PartialEq)]
pub struct CoreEvent {
    pub ts_ms: i64,
    /// pane.output, api.call, policy.decision, agent.state, client.attach, hook.tool …
    pub kind: String,
    /// 누가 일으켰나(user, w1:p2, plugin:name, server)
    pub source: String,
    /// 본문. 마스킹 필터는 문자열 값을 모두 검사한다.
    pub data: Value,
    /// 필터가 덧붙이는 표시(예: "redacted:2")
    pub tags: Vec<String>,
}

impl CoreEvent {
    pub fn new(kind: &str, source: &str, data: Value) -> Self {
        Self {
            ts_ms: kmttt_proto::now_ms(),
            kind: kind.into(),
            source: source.into(),
            data,
            tags: Vec::new(),
        }
    }
}

/// 필터 판단.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterAction {
    /// 다음 필터로 넘김
    Continue,
    /// 여기서 버림(뒤 필터와 구독자에게 가지 않음)
    Drop,
}

pub trait Filter: Send + Sync {
    fn name(&self) -> &'static str;
    fn process(&self, ev: &mut CoreEvent) -> FilterAction;
}

/// 아무것도 하지 않는 필터(테스트·초기 단계용).
pub struct NoopFilter;

impl Filter for NoopFilter {
    fn name(&self) -> &'static str {
        "noop"
    }
    fn process(&self, _ev: &mut CoreEvent) -> FilterAction {
        FilterAction::Continue
    }
}

#[derive(Default)]
pub struct FilterChain {
    filters: Vec<Box<dyn Filter>>,
}

impl FilterChain {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, f: Box<dyn Filter>) {
        self.filters.push(f);
    }

    pub fn names(&self) -> Vec<&'static str> {
        self.filters.iter().map(|f| f.name()).collect()
    }

    /// 이벤트를 모든 필터에 통과시킨다. 버려지면 None.
    pub fn process(&self, mut ev: CoreEvent) -> Option<CoreEvent> {
        for f in &self.filters {
            if f.process(&mut ev) == FilterAction::Drop {
                return None;
            }
        }
        Some(ev)
    }
}

/// JSON 값 안의 모든 문자열에 함수를 적용한다(마스킹 필터가 쓴다).
pub fn map_strings(v: &mut Value, f: &mut dyn FnMut(&mut String)) {
    match v {
        Value::String(s) => f(s),
        Value::Array(a) => a.iter_mut().for_each(|x| map_strings(x, f)),
        Value::Object(o) => o.values_mut().for_each(|x| map_strings(x, f)),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct Upper;
    impl Filter for Upper {
        fn name(&self) -> &'static str {
            "upper"
        }
        fn process(&self, ev: &mut CoreEvent) -> FilterAction {
            map_strings(&mut ev.data, &mut |s| *s = s.to_uppercase());
            FilterAction::Continue
        }
    }
    struct DropAll;
    impl Filter for DropAll {
        fn name(&self) -> &'static str {
            "drop"
        }
        fn process(&self, _: &mut CoreEvent) -> FilterAction {
            FilterAction::Drop
        }
    }

    #[test]
    fn chain_order_and_drop() {
        let mut c = FilterChain::new();
        c.push(Box::new(NoopFilter));
        c.push(Box::new(Upper));
        let out = c
            .process(CoreEvent::new("x", "s", json!({"a": ["b", {"c": "d"}]})))
            .unwrap();
        assert_eq!(out.data, json!({"a": ["B", {"c": "D"}]}));
        c.push(Box::new(DropAll));
        assert!(c.process(CoreEvent::new("x", "s", json!("y"))).is_none());
        assert_eq!(c.names(), vec!["noop", "upper", "drop"]);
    }
}
