//! Tagged-JSON codec for values crossing the sandbox boundary.
//!
//! Plain JSON values map to themselves. Anything JSON can't express is an
//! object with a reserved `"$t"` key:
//!
//! | tag  | meaning                                   | payload                       |
//! |------|-------------------------------------------|-------------------------------|
//! | `u`  | undefined                                 | —                             |
//! | `n`  | NaN / Infinity / -Infinity / -0           | `v`: string                   |
//! | `bi` | BigInt                                    | `v`: decimal string           |
//! | `d`  | Date                                      | `v`: epoch ms, null = invalid |
//! | `b`  | bytes (ArrayBuffer / typed array)         | `v`: base64, `k`: kind        |
//! | `m`  | Map                                       | `v`: [[k, v], ...]            |
//! | `s`  | Set                                       | `v`: [...]                    |
//! | `re` | RegExp                                    | `src`, `flags`                |
//! | `e`  | Error                                     | `name`, `message`, `stack`... |
//! | `o`  | plain object that itself has a `$t` key   | `v`: object                   |
//!
//! The walk happens in compiled Rust against the QuickJS C API, so it is not
//! interpreted JS and does not burn guest fuel per value.

use base64::Engine as _;
use rquickjs::{qjs, Array, Ctx, Function, Object, Type, TypedArray, Value};
use serde_json::Value as Json;

pub struct Helpers<'js> {
    pub entries: Function<'js>,
    pub values: Function<'js>,
    pub mk_map: Function<'js>,
    pub mk_set: Function<'js>,
    pub mk_regexp: Function<'js>,
    pub mk_bigint: Function<'js>,
    pub mk_error: Function<'js>,
    pub view_bytes: Function<'js>,
    pub mk_view: Function<'js>,
    pub ctor_name: Function<'js>,
}

impl<'js> Helpers<'js> {
    pub fn new(hooks: Object<'js>) -> Self {
        let f = |k: &str| hooks.get::<_, Function>(k).expect("prelude hook");
        Helpers {
            entries: f("entries"),
            values: f("values"),
            mk_map: f("mkMap"),
            mk_set: f("mkSet"),
            mk_regexp: f("mkRegExp"),
            mk_bigint: f("mkBigInt"),
            mk_error: f("mkError"),
            view_bytes: f("viewBytes"),
            mk_view: f("mkView"),
            ctor_name: f("ctorName"),
        }
    }
}

/// Why a value could not be encoded, plus where in the value it happened.
pub struct EncodeError {
    reason: String,
    /// Path segments, innermost first (pushed while unwinding).
    path: Vec<String>,
}

impl EncodeError {
    fn new(reason: impl Into<String>) -> Self {
        EncodeError { reason: reason.into(), path: Vec::new() }
    }

    fn at(mut self, seg: String) -> Self {
        self.path.push(seg);
        self
    }

    fn path_from(&self, root: &str) -> String {
        let mut s = root.to_owned();
        for seg in self.path.iter().rev() {
            s.push_str(seg);
        }
        s
    }

    /// e.g. "arguments[0].handler is a function, which cannot cross the sandbox boundary"
    pub fn describe(&self, root: &str) -> String {
        format!("{} {}", self.path_from(root), self.reason)
    }

    /// e.g. "contains a function at .items[2]" / "is a function"
    pub fn describe_bare(&self) -> String {
        if self.path.is_empty() {
            self.reason.clone()
        } else {
            format!("{} (at {})", self.reason, self.path_from(""))
        }
    }
}

type EResult = Result<(), EncodeError>;

pub fn encode<'js>(
    ctx: &Ctx<'js>,
    v: &Value<'js>,
    h: &Helpers<'js>,
    out: &mut Vec<u8>,
    limit: usize,
    max_depth: usize,
) -> EResult {
    let mut enc = Encoder { ctx, h, out, limit, max_depth, stack: Vec::new() };
    enc.value(v)
}

struct Encoder<'a, 'js> {
    ctx: &'a Ctx<'js>,
    h: &'a Helpers<'js>,
    out: &'a mut Vec<u8>,
    limit: usize,
    max_depth: usize,
    /// Addresses of objects on the current path, for cycle detection.
    stack: Vec<usize>,
}

fn key_seg(key: &str) -> String {
    let ident = !key.is_empty()
        && key.chars().enumerate().all(|(i, c)| c == '_' || c == '$' || c.is_ascii_alphabetic() || (i > 0 && c.is_ascii_digit()));
    if ident {
        format!(".{key}")
    } else {
        format!("[{}]", serde_json::to_string(key).unwrap())
    }
}

/// Borrow a typed array's bytes. Only valid until JS runs again, so callers
/// copy out immediately.
pub fn bytes_of<'a>(view: &'a TypedArray<'_, u8>) -> &'a [u8] {
    match view.as_raw() {
        Some(raw) => unsafe { &*raw.as_ptr() },
        None => &[],
    }
}

fn raw_ptr(v: &Value<'_>) -> usize {
    unsafe { qjs::JS_VALUE_GET_PTR(v.as_raw()) as usize }
}

impl<'a, 'js> Encoder<'a, 'js> {
    fn check_size(&self) -> EResult {
        if self.out.len() > self.limit {
            return Err(EncodeError::new(format!("is larger than the {} byte limit", self.limit)));
        }
        Ok(())
    }

    fn raw(&mut self, s: &str) {
        self.out.extend_from_slice(s.as_bytes());
    }

    fn string(&mut self, s: &str) {
        serde_json::to_writer(&mut *self.out, s).unwrap();
    }

    fn number(&mut self, f: f64) {
        if f.is_nan() {
            self.raw(r#"{"$t":"n","v":"NaN"}"#)
        } else if f.is_infinite() {
            self.raw(if f > 0.0 { r#"{"$t":"n","v":"Infinity"}"# } else { r#"{"$t":"n","v":"-Infinity"}"# })
        } else if f == 0.0 && f.is_sign_negative() {
            self.raw(r#"{"$t":"n","v":"-0"}"#)
        } else if f.fract() == 0.0 && f.abs() < 9_007_199_254_740_992.0 {
            self.raw(&(f as i64).to_string())
        } else {
            serde_json::to_writer(&mut *self.out, &f).unwrap()
        }
    }

    fn tagged(&mut self, tag: &str) {
        self.raw("{\"$t\":\"");
        self.raw(tag);
        self.raw("\"");
    }

    fn value(&mut self, v: &Value<'js>) -> EResult {
        self.check_size()?;
        match v.type_of() {
            Type::Undefined | Type::Uninitialized => self.raw(r#"{"$t":"u"}"#),
            Type::Null => self.raw("null"),
            Type::Bool => self.raw(if v.as_bool().unwrap() { "true" } else { "false" }),
            Type::Int => self.raw(&v.as_int().unwrap().to_string()),
            Type::Float => self.number(v.as_float().unwrap()),
            Type::String => {
                let s = v.as_string().unwrap().to_string().map_err(|_| EncodeError::new("is not a valid string"))?;
                self.string(&s)
            }
            Type::BigInt => {
                let s: rquickjs::Coerced<String> =
                    rquickjs::FromJs::from_js(self.ctx, v.clone()).map_err(|_| EncodeError::new("is an unreadable BigInt"))?;
                self.tagged("bi");
                self.raw(",\"v\":");
                self.string(&s.0);
                self.raw("}");
            }
            Type::Symbol => return Err(EncodeError::new("is a Symbol, which cannot cross the sandbox boundary")),
            Type::Function | Type::Constructor => {
                return Err(EncodeError::new("is a function, which cannot cross the sandbox boundary"))
            }
            Type::Promise => {
                return Err(EncodeError::new("is a Promise, which cannot cross the sandbox boundary (missing `await`?)"))
            }
            Type::Array => self.array(v)?,
            Type::Exception => self.error(v.as_object().unwrap())?,
            Type::Object | Type::Proxy => self.object(v)?,
            _ => return Err(EncodeError::new(format!("has unsupported type {}", v.type_name()))),
        }
        Ok(())
    }

    fn enter(&mut self, v: &Value<'js>) -> EResult {
        let p = raw_ptr(v);
        if self.stack.contains(&p) {
            return Err(EncodeError::new("contains a circular reference"));
        }
        if self.stack.len() >= self.max_depth {
            return Err(EncodeError::new(format!("is nested deeper than {} levels", self.max_depth)));
        }
        self.stack.push(p);
        Ok(())
    }

    fn array(&mut self, v: &Value<'js>) -> EResult {
        self.enter(v)?;
        let arr = v.as_array().unwrap();
        self.raw("[");
        for i in 0..arr.len() {
            if i > 0 {
                self.raw(",");
            }
            let item: Value = arr.get(i).map_err(|_| EncodeError::new("could not be read").at(format!("[{i}]")))?;
            self.value(&item).map_err(|e| e.at(format!("[{i}]")))?;
        }
        self.raw("]");
        self.stack.pop();
        Ok(())
    }

    fn list(&mut self, items: Array<'js>, pair: bool) -> EResult {
        self.raw("[");
        for i in 0..items.len() {
            if i > 0 {
                self.raw(",");
            }
            let item: Value = items.get(i).map_err(|_| EncodeError::new("could not be read"))?;
            if pair {
                let kv = item.as_array().unwrap();
                self.raw("[");
                self.value(&kv.get(0).unwrap()).map_err(|e| e.at(format!(".<key {i}>")))?;
                self.raw(",");
                self.value(&kv.get(1).unwrap()).map_err(|e| e.at(format!(".<value {i}>")))?;
                self.raw("]");
            } else {
                self.value(&item).map_err(|e| e.at(format!(".<item {i}>")))?;
            }
        }
        self.raw("]");
        Ok(())
    }

    fn error(&mut self, obj: &Object<'js>) -> EResult {
        let get = |k: &str| obj.get::<_, Option<String>>(k).ok().flatten();
        self.tagged("e");
        self.raw(",\"name\":");
        self.string(&get("name").unwrap_or_else(|| "Error".into()));
        self.raw(",\"message\":");
        self.string(&get("message").unwrap_or_default());
        if let Some(stack) = get("stack") {
            self.raw(",\"stack\":");
            self.string(&stack);
        }
        if let Ok(code) = obj.get::<_, Value>("code") {
            if !code.is_undefined() {
                self.raw(",\"code\":");
                self.value(&code).map_err(|e| e.at(".code".into()))?;
            }
        }
        if let Ok(cause) = obj.get::<_, Value>("cause") {
            if !cause.is_undefined() {
                self.enter(obj.as_value())?;
                self.raw(",\"cause\":");
                self.value(&cause).map_err(|e| e.at(".cause".into()))?;
                self.stack.pop();
            }
        }
        self.raw("}");
        Ok(())
    }

    fn bytes(&mut self, kind: &str, data: &[u8]) {
        self.tagged("b");
        if kind != "Uint8Array" {
            self.raw(",\"k\":");
            self.string(kind);
        }
        self.raw(",\"v\":\"");
        let encoded = base64::engine::general_purpose::STANDARD.encode(data);
        self.raw(&encoded);
        self.raw("\"}");
    }

    fn object(&mut self, v: &Value<'js>) -> EResult {
        let raw = v.as_raw();
        let ctx_ptr = self.ctx.as_raw().as_ptr();
        let obj = v.as_object().unwrap();
        unsafe {
            if qjs::JS_IsDate(raw) {
                let get_time: Function = obj.get("getTime").map_err(|_| EncodeError::new("is a broken Date"))?;
                let t: f64 = get_time
                    .call((rquickjs::function::This(v.clone()),))
                    .map_err(|_| EncodeError::new("is a broken Date"))?;
                self.tagged("d");
                self.raw(",\"v\":");
                if t.is_finite() {
                    self.raw(&(t as i64).to_string());
                } else {
                    self.raw("null");
                }
                self.raw("}");
                return Ok(());
            }
            if qjs::JS_IsError(raw) {
                return self.error(obj);
            }
            if qjs::JS_IsRegExp(raw) {
                let src: String = obj.get("source").unwrap_or_default();
                let flags: String = obj.get("flags").unwrap_or_default();
                self.tagged("re");
                self.raw(",\"src\":");
                self.string(&src);
                self.raw(",\"flags\":");
                self.string(&flags);
                self.raw("}");
                return Ok(());
            }
            if qjs::JS_IsMap(raw) || qjs::JS_IsSet(raw) {
                let is_map = qjs::JS_IsMap(raw);
                self.enter(v)?;
                let f = if is_map { &self.h.entries } else { &self.h.values };
                let items: Array = f.call((v.clone(),)).map_err(|_| EncodeError::new("could not be iterated"))?;
                self.tagged(if is_map { "m" } else { "s" });
                self.raw(",\"v\":");
                self.list(items, is_map)?;
                self.raw("}");
                self.stack.pop();
                return Ok(());
            }
            if qjs::JS_IsArrayBuffer(raw) {
                let mut size = 0usize;
                let ptr = qjs::JS_GetArrayBuffer(ctx_ptr, &mut size as *mut usize as *mut _, raw);
                let data = if ptr.is_null() { &[][..] } else { std::slice::from_raw_parts(ptr, size) };
                self.bytes("ArrayBuffer", data);
                return Ok(());
            }
            let ta = qjs::JS_GetTypedArrayType(raw);
            if ta >= 0 {
                let kind = match ta as u32 {
                    0 => "Uint8ClampedArray",
                    1 => "Int8Array",
                    2 => "Uint8Array",
                    3 => "Int16Array",
                    4 => "Uint16Array",
                    5 => "Int32Array",
                    6 => "Uint32Array",
                    7 => "BigInt64Array",
                    8 => "BigUint64Array",
                    9 => "Float16Array",
                    10 => "Float32Array",
                    _ => "Float64Array",
                };
                let view: TypedArray<u8> = if ta == 2 {
                    TypedArray::from_value(v.clone()).map_err(|_| EncodeError::new("is a detached buffer"))?
                } else {
                    self.h.view_bytes.call((v.clone(),)).map_err(|_| EncodeError::new("is a detached buffer"))?
                };
                self.bytes(kind, bytes_of(&view));
                return Ok(());
            }
            if qjs::JS_IsPromise(raw) {
                return Err(EncodeError::new("is a Promise, which cannot cross the sandbox boundary (missing `await`?)"));
            }
        }

        self.enter(v)?;
        let mut keys: Vec<String> = Vec::new();
        for key in obj.keys::<String>() {
            keys.push(key.map_err(|_| EncodeError::new("has an unreadable key"))?);
        }
        let escaped = keys.iter().any(|k| k == "$t");
        if escaped {
            self.raw(r#"{"$t":"o","v":"#);
        }
        self.raw("{");
        let mut first = true;
        for key in &keys {
            let item: Value = obj.get(key.as_str()).map_err(|_| EncodeError::new("could not be read").at(key_seg(key)))?;
            if !first {
                self.raw(",");
            }
            first = false;
            self.string(key);
            self.raw(":");
            self.value(&item).map_err(|e| e.at(key_seg(key)))?;
        }
        self.raw("}");
        if escaped {
            self.raw("}");
        }
        self.stack.pop();
        Ok(())
    }
}

// ---- decode -------------------------------------------------------------------

pub fn decode<'js>(ctx: &Ctx<'js>, j: &Json, h: &Helpers<'js>) -> rquickjs::Result<Value<'js>> {
    Ok(match j {
        Json::Null => Value::new_null(ctx.clone()),
        Json::Bool(b) => Value::new_bool(ctx.clone(), *b),
        Json::Number(n) => match n.as_i64() {
            Some(i) if i >= i32::MIN as i64 && i <= i32::MAX as i64 => Value::new_int(ctx.clone(), i as i32),
            _ => Value::new_number(ctx.clone(), n.as_f64().unwrap_or(f64::NAN)),
        },
        Json::String(s) => rquickjs::String::from_str(ctx.clone(), s)?.into_value(),
        Json::Array(items) => {
            let arr = Array::new(ctx.clone())?;
            for (i, item) in items.iter().enumerate() {
                arr.set(i, decode(ctx, item, h)?)?;
            }
            arr.into_value()
        }
        Json::Object(map) => match map.get("$t").and_then(Json::as_str) {
            None => plain_object(ctx, map, h)?,
            Some(tag) => decode_tagged(ctx, tag, map, h)?,
        },
    })
}

fn plain_object<'js>(
    ctx: &Ctx<'js>,
    map: &serde_json::Map<String, Json>,
    h: &Helpers<'js>,
) -> rquickjs::Result<Value<'js>> {
    let obj = Object::new(ctx.clone())?;
    for (k, v) in map {
        obj.set(k.as_str(), decode(ctx, v, h)?)?;
    }
    Ok(obj.into_value())
}

fn decode_tagged<'js>(
    ctx: &Ctx<'js>,
    tag: &str,
    map: &serde_json::Map<String, Json>,
    h: &Helpers<'js>,
) -> rquickjs::Result<Value<'js>> {
    let v = map.get("v").unwrap_or(&Json::Null);
    let s = |k: &str| map.get(k).and_then(Json::as_str).unwrap_or("");
    Ok(match tag {
        "u" => Value::new_undefined(ctx.clone()),
        "n" => Value::new_float(
            ctx.clone(),
            match v.as_str().unwrap_or("NaN") {
                "Infinity" => f64::INFINITY,
                "-Infinity" => f64::NEG_INFINITY,
                "-0" => -0.0,
                _ => f64::NAN,
            },
        ),
        "bi" => h.mk_bigint.call((v.as_str().unwrap_or("0"),))?,
        "d" => unsafe {
            let ms = v.as_f64().unwrap_or(f64::NAN);
            let raw = qjs::JS_NewDate(ctx.as_raw().as_ptr(), ms);
            Value::from_raw(ctx.clone(), raw)
        },
        "b" => {
            let bytes = base64::engine::general_purpose::STANDARD.decode(v.as_str().unwrap_or("")).unwrap_or_default();
            let kind = map.get("k").and_then(Json::as_str).unwrap_or("Uint8Array");
            let u8s = TypedArray::<u8>::new(ctx.clone(), bytes)?;
            match kind {
                "Uint8Array" => u8s.into_value(),
                "ArrayBuffer" => u8s.arraybuffer()?.into_value(),
                other => h.mk_view.call((other, u8s))?,
            }
        }
        "m" | "s" => {
            let arr = match v {
                Json::Array(_) => decode(ctx, v, h)?,
                _ => Array::new(ctx.clone())?.into_value(),
            };
            if tag == "m" { h.mk_map.call((arr,))? } else { h.mk_set.call((arr,))? }
        }
        "re" => h.mk_regexp.call((s("src"), s("flags")))?,
        "e" => {
            let mut extra = serde_json::Map::new();
            for k in ["code", "cause", "data"] {
                if let Some(x) = map.get(k) {
                    extra.insert(k.into(), x.clone());
                }
            }
            let extra = plain_object(ctx, &extra, h)?;
            let stack = map.get("stack").and_then(Json::as_str);
            let stack = match stack {
                Some(st) => rquickjs::String::from_str(ctx.clone(), st)?.into_value(),
                None => Value::new_undefined(ctx.clone()),
            };
            h.mk_error.call((s("name"), s("message"), stack, extra))?
        }
        "o" => match v {
            Json::Object(inner) => plain_object(ctx, inner, h)?,
            _ => plain_object(ctx, map, h)?,
        },
        _ => plain_object(ctx, map, h)?,
    })
}
