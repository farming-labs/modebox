//! Values that cross the sandbox boundary.
//!
//! On the wire this is "tagged JSON": plain JSON for plain data, plus objects
//! with a reserved `"$t"` key for everything JSON can't express (see the guest's
//! `codec.rs` for the table). [`Value`] is the host-side model of that format.

use base64::Engine as _;
use indexmap::IndexMap;
use serde_json::{Map, Value as Json, json};

#[derive(Clone, Debug, PartialEq, Default)]
pub enum Value {
    #[default]
    Undefined,
    Null,
    Bool(bool),
    Number(f64),
    /// Decimal string, so values beyond i64 survive.
    BigInt(String),
    String(String),
    /// Raw bytes plus the JS kind they came from (`Uint8Array`, `ArrayBuffer`, ...).
    Bytes { kind: String, data: Vec<u8> },
    /// Epoch milliseconds; `None` is an invalid Date.
    Date(Option<f64>),
    Array(Vec<Value>),
    Object(IndexMap<String, Value>),
    Map(Vec<(Value, Value)>),
    Set(Vec<Value>),
    RegExp { source: String, flags: String },
    Error(ErrorValue),
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct ErrorValue {
    pub name: String,
    pub message: String,
    pub stack: Option<String>,
    pub code: Option<Box<Value>>,
    pub cause: Option<Box<Value>>,
}

impl Value {
    pub fn object<K: Into<String>>(entries: impl IntoIterator<Item = (K, Value)>) -> Value {
        Value::Object(entries.into_iter().map(|(k, v)| (k.into(), v)).collect())
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Number(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Object(m) => m.get(key),
            _ => None,
        }
    }

    pub fn is_undefined(&self) -> bool {
        matches!(self, Value::Undefined)
    }

    /// Decode from the tagged wire format.
    pub fn from_tagged(j: &Json) -> Value {
        match j {
            Json::Null => Value::Null,
            Json::Bool(b) => Value::Bool(*b),
            Json::Number(n) => Value::Number(n.as_f64().unwrap_or(f64::NAN)),
            Json::String(s) => Value::String(s.clone()),
            Json::Array(a) => Value::Array(a.iter().map(Value::from_tagged).collect()),
            Json::Object(m) => match m.get("$t").and_then(Json::as_str) {
                None => Value::Object(m.iter().map(|(k, v)| (k.clone(), Value::from_tagged(v))).collect()),
                Some(tag) => Self::from_tag(tag, m),
            },
        }
    }

    fn from_tag(tag: &str, m: &Map<String, Json>) -> Value {
        let v = m.get("v").unwrap_or(&Json::Null);
        let s = |k: &str| m.get(k).and_then(Json::as_str).unwrap_or("").to_owned();
        match tag {
            "u" => Value::Undefined,
            "n" => Value::Number(match v.as_str() {
                Some("Infinity") => f64::INFINITY,
                Some("-Infinity") => f64::NEG_INFINITY,
                Some("-0") => -0.0,
                _ => f64::NAN,
            }),
            "bi" => Value::BigInt(v.as_str().unwrap_or("0").to_owned()),
            "d" => Value::Date(v.as_f64()),
            "b" => Value::Bytes {
                kind: m.get("k").and_then(Json::as_str).unwrap_or("Uint8Array").to_owned(),
                data: base64::engine::general_purpose::STANDARD
                    .decode(v.as_str().unwrap_or(""))
                    .unwrap_or_default(),
            },
            "m" => Value::Map(
                v.as_array()
                    .map(|pairs| {
                        pairs
                            .iter()
                            .filter_map(|p| {
                                let p = p.as_array()?;
                                Some((Value::from_tagged(p.first()?), Value::from_tagged(p.get(1)?)))
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
            ),
            "s" => Value::Set(v.as_array().map(|a| a.iter().map(Value::from_tagged).collect()).unwrap_or_default()),
            "re" => Value::RegExp { source: s("src"), flags: s("flags") },
            "e" => Value::Error(ErrorValue {
                name: s("name"),
                message: s("message"),
                stack: m.get("stack").and_then(Json::as_str).map(str::to_owned),
                code: m.get("code").map(|c| Box::new(Value::from_tagged(c))),
                cause: m.get("cause").map(|c| Box::new(Value::from_tagged(c))),
            }),
            "o" => match v {
                Json::Object(inner) => {
                    Value::Object(inner.iter().map(|(k, v)| (k.clone(), Value::from_tagged(v))).collect())
                }
                _ => Value::Null,
            },
            _ => Value::Object(m.iter().map(|(k, v)| (k.clone(), Value::from_tagged(v))).collect()),
        }
    }

    /// Encode to the tagged wire format (lossless).
    pub fn to_tagged(&self) -> Json {
        match self {
            Value::Undefined => json!({"$t": "u"}),
            Value::Null => Json::Null,
            Value::Bool(b) => Json::Bool(*b),
            Value::Number(n) => number_tagged(*n),
            Value::BigInt(s) => json!({"$t": "bi", "v": s}),
            Value::String(s) => Json::String(s.clone()),
            Value::Bytes { kind, data } => {
                let mut m = Map::new();
                m.insert("$t".into(), "b".into());
                if kind != "Uint8Array" {
                    m.insert("k".into(), kind.clone().into());
                }
                m.insert("v".into(), base64::engine::general_purpose::STANDARD.encode(data).into());
                Json::Object(m)
            }
            Value::Date(t) => json!({"$t": "d", "v": t}),
            Value::Array(a) => Json::Array(a.iter().map(Value::to_tagged).collect()),
            Value::Object(m) => {
                let inner: Map<String, Json> = m.iter().map(|(k, v)| (k.clone(), v.to_tagged())).collect();
                if m.contains_key("$t") {
                    json!({"$t": "o", "v": inner})
                } else {
                    Json::Object(inner)
                }
            }
            Value::Map(pairs) => {
                json!({"$t": "m", "v": pairs.iter().map(|(k, v)| json!([k.to_tagged(), v.to_tagged()])).collect::<Vec<_>>()})
            }
            Value::Set(items) => json!({"$t": "s", "v": items.iter().map(Value::to_tagged).collect::<Vec<_>>()}),
            Value::RegExp { source, flags } => json!({"$t": "re", "src": source, "flags": flags}),
            Value::Error(e) => {
                let mut m = Map::new();
                m.insert("$t".into(), "e".into());
                m.insert("name".into(), e.name.clone().into());
                m.insert("message".into(), e.message.clone().into());
                if let Some(st) = &e.stack {
                    m.insert("stack".into(), st.clone().into());
                }
                if let Some(c) = &e.code {
                    m.insert("code".into(), c.to_tagged());
                }
                if let Some(c) = &e.cause {
                    m.insert("cause".into(), c.to_tagged());
                }
                Json::Object(m)
            }
        }
    }

    /// Plain JSON for display or for callers that only speak JSON. Lossy in the
    /// same places `JSON.stringify` is: undefined becomes null (or is dropped in
    /// objects), Dates become ISO strings, Maps become objects or entry lists.
    pub fn to_json(&self) -> Json {
        match self {
            Value::Undefined | Value::Null => Json::Null,
            Value::Bool(b) => Json::Bool(*b),
            Value::Number(n) => {
                if n.is_finite() {
                    if n.fract() == 0.0 && n.abs() < 9_007_199_254_740_992.0 {
                        Json::from(*n as i64)
                    } else {
                        serde_json::Number::from_f64(*n).map(Json::Number).unwrap_or(Json::Null)
                    }
                } else {
                    Json::Null
                }
            }
            Value::BigInt(s) => Json::String(s.clone()),
            Value::String(s) => Json::String(s.clone()),
            Value::Bytes { data, .. } => Json::String(base64::engine::general_purpose::STANDARD.encode(data)),
            Value::Date(t) => match t.and_then(iso8601) {
                Some(s) => Json::String(s),
                None => Json::Null,
            },
            Value::Array(a) => Json::Array(a.iter().map(Value::to_json).collect()),
            Value::Object(m) => {
                Json::Object(m.iter().filter(|(_, v)| !v.is_undefined()).map(|(k, v)| (k.clone(), v.to_json())).collect())
            }
            Value::Map(pairs) => {
                if pairs.iter().all(|(k, _)| matches!(k, Value::String(_))) {
                    Json::Object(pairs.iter().map(|(k, v)| (k.as_str().unwrap().to_owned(), v.to_json())).collect())
                } else {
                    Json::Array(pairs.iter().map(|(k, v)| json!([k.to_json(), v.to_json()])).collect())
                }
            }
            Value::Set(items) => Json::Array(items.iter().map(Value::to_json).collect()),
            Value::RegExp { source, flags } => Json::String(format!("/{source}/{flags}")),
            Value::Error(e) => json!({"name": e.name, "message": e.message}),
        }
    }
}

fn number_tagged(n: f64) -> Json {
    if n.is_nan() {
        json!({"$t": "n", "v": "NaN"})
    } else if n.is_infinite() {
        json!({"$t": "n", "v": if n > 0.0 { "Infinity" } else { "-Infinity" }})
    } else if n == 0.0 && n.is_sign_negative() {
        json!({"$t": "n", "v": "-0"})
    } else if n.fract() == 0.0 && n.abs() < 9_007_199_254_740_992.0 {
        Json::from(n as i64)
    } else {
        serde_json::Number::from_f64(n).map(Json::Number).unwrap_or(Json::Null)
    }
}

/// Format epoch milliseconds as `YYYY-MM-DDTHH:MM:SS.mmmZ` (proleptic Gregorian).
fn iso8601(ms: f64) -> Option<String> {
    if !ms.is_finite() {
        return None;
    }
    let ms = ms as i64;
    let days = ms.div_euclid(86_400_000);
    let rem = ms.rem_euclid(86_400_000);
    // Civil-from-days (Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    Some(format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{:03}Z",
        rem / 3_600_000,
        rem / 60_000 % 60,
        rem / 1000 % 60,
        rem % 1000
    ))
}

impl From<Json> for Value {
    /// Plain JSON in (no tag interpretation).
    fn from(j: Json) -> Value {
        match j {
            Json::Null => Value::Null,
            Json::Bool(b) => Value::Bool(b),
            Json::Number(n) => Value::Number(n.as_f64().unwrap_or(f64::NAN)),
            Json::String(s) => Value::String(s),
            Json::Array(a) => Value::Array(a.into_iter().map(Value::from).collect()),
            Json::Object(m) => Value::Object(m.into_iter().map(|(k, v)| (k, Value::from(v))).collect()),
        }
    }
}

macro_rules! from_num {
    ($($t:ty),*) => {$(
        impl From<$t> for Value {
            fn from(n: $t) -> Value { Value::Number(n as f64) }
        }
    )*};
}
from_num!(i8, i16, i32, i64, u8, u16, u32, u64, usize, isize, f32, f64);

impl From<bool> for Value {
    fn from(b: bool) -> Value {
        Value::Bool(b)
    }
}

impl From<&str> for Value {
    fn from(s: &str) -> Value {
        Value::String(s.to_owned())
    }
}

impl From<String> for Value {
    fn from(s: String) -> Value {
        Value::String(s)
    }
}

impl<T: Into<Value>> From<Vec<T>> for Value {
    fn from(v: Vec<T>) -> Value {
        Value::Array(v.into_iter().map(Into::into).collect())
    }
}

impl<T: Into<Value>> From<Option<T>> for Value {
    fn from(v: Option<T>) -> Value {
        v.map(Into::into).unwrap_or(Value::Null)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tagged_roundtrip() {
        let v = Value::object([
            ("n", Value::Number(1.5)),
            ("nan", Value::Number(f64::NAN)),
            ("big", Value::BigInt("123456789012345678901234567890".into())),
            ("d", Value::Date(Some(0.0))),
            ("b", Value::Bytes { kind: "Uint8Array".into(), data: vec![1, 2, 3] }),
            ("m", Value::Map(vec![(Value::from("k"), Value::from(1))])),
            ("$t", Value::from("user data with a reserved key")),
        ]);
        let back = Value::from_tagged(&v.to_tagged());
        // NaN != NaN, so compare everything but that field.
        let (Value::Object(a), Value::Object(b)) = (&v, &back) else { panic!() };
        for (k, x) in a {
            if k != "nan" {
                assert_eq!(Some(x), b.get(k), "{k}");
            }
        }
        assert!(b["nan"].as_f64().unwrap().is_nan());
    }

    #[test]
    fn iso_dates() {
        assert_eq!(iso8601(0.0).unwrap(), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso8601(1_700_000_000_123.0).unwrap(), "2023-11-14T22:13:20.123Z");
        assert_eq!(iso8601(-1.0).unwrap(), "1969-12-31T23:59:59.999Z");
    }
}
