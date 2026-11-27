//! 정규화된 JSON: 객체 키를 사전순으로 정렬하고 공백 없이 쓴다.
//! serde_json의 Map 순서(preserve_order 기능 여부)에 기대지 않는다.

use serde_json::Value;

pub fn canonical(v: &Value) -> String {
    let mut out = String::new();
    write(v, &mut out);
    out
}

fn write(v: &Value, out: &mut String) {
    match v {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, k) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::to_string(k).expect("string"));
                out.push(':');
                write(&map[*k], out);
            }
            out.push('}');
        }
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write(x, out);
            }
            out.push(']');
        }
        other => out.push_str(&serde_json::to_string(other).expect("scalar")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sorted_and_compact() {
        let v = json!({"b": 1, "a": {"d": [1, "x"], "c": null}});
        assert_eq!(canonical(&v), r#"{"a":{"c":null,"d":[1,"x"]},"b":1}"#);
    }

    #[test]
    fn escapes() {
        assert_eq!(canonical(&json!({"k": "a\"b\n"})), r#"{"k":"a\"b\n"}"#);
    }
}
