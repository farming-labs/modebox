//! Tagged-JSON codec for values crossing the sandbox boundary.
//!
//! Plain JSON values map to themselves. Anything JSON can't express is an
//! object whose first key is a reserved `"$t"`:
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
//!
//! Nothing here may abort the guest: a failed Rust allocation in wasm is a
//! trap that loses the whole run. So the encoder borrows string data from
//! QuickJS instead of copying it, checks the size limit before every write
//! and grows its buffer fallibly, and the decoder builds JS values straight
//! from the JSON text instead of parsing it into a tree first.

use std::borrow::Cow;
use std::fmt;

use base64::Engine as _;
use rquickjs::object::Property;
use rquickjs::{qjs, Array, Atom, CString, Ctx, Function, Object, Type, TypedArray, Value};
use serde::de::{self, DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor};

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
    /// Symbol of a method that gives the value to send instead (a URL's href).
    pub encode_as: Value<'js>,
}

impl<'js> Helpers<'js> {
    /// `hooks` is the object the prelude returns. Guest code can't reach it,
    /// so a missing hook is a bug in the prelude, not something input causes.
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
            encode_as: hooks.get("encodeAs").expect("prelude hook"),
        }
    }
}

// ---- strings ----------------------------------------------------------------

/// A JS string's UTF-8, borrowed from QuickJS. Pure-ASCII strings aren't
/// copied at all; others are converted inside QuickJS's heap, where running
/// out of memory is an ordinary catchable error.
pub struct Utf8<'js>(CString<'js>);

impl Utf8<'_> {
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

pub enum StrError {
    /// Contains an unpaired surrogate, which UTF-8 can't represent.
    NotUtf8,
    OutOfMemory,
}

impl StrError {
    pub fn describe(&self) -> &'static str {
        match self {
            StrError::NotUtf8 => "is not a valid string",
            StrError::OutOfMemory => "is too large for the sandbox's memory",
        }
    }
}

/// UTF-8 of a JS string value (`None` if `v` isn't a string).
pub fn utf8<'js>(ctx: &Ctx<'js>, v: &Value<'js>) -> Option<Result<Utf8<'js>, StrError>> {
    let s = v.as_string()?.clone();
    Some(match s.to_cstring() {
        Ok(c) => {
            let bytes = unsafe { std::slice::from_raw_parts(c.as_ptr() as *const u8, c.len()) };
            if std::str::from_utf8(bytes).is_ok() {
                Ok(Utf8(c))
            } else {
                Err(StrError::NotUtf8)
            }
        }
        Err(_) => {
            let _ = ctx.catch();
            Err(StrError::OutOfMemory)
        }
    })
}

/// QuickJS writes an unpaired surrogate as the 3-byte sequence of its code
/// point (WTF-8), which isn't UTF-8. Replace each with U+FFFD, as a UTF-8
/// encoder does; it's also 3 bytes, so the length doesn't change. `None` if
/// the copy can't be allocated.
pub fn replace_surrogates(bytes: &[u8]) -> Option<String> {
    let mut out = Vec::new();
    out.try_reserve_exact(bytes.len()).ok()?;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 0xED && i + 2 < bytes.len() && (0xA0..=0xBF).contains(&bytes[i + 1]) {
            out.extend_from_slice("\u{FFFD}".as_bytes());
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    // Anything else invalid would be a QuickJS bug; don't trust it either way.
    Some(String::from_utf8(out).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned()))
}

/// At most `max` bytes of a JS string as an owned Rust string, cut on a char
/// boundary, plus whether it was cut. Unpaired surrogates become U+FFFD.
pub fn clip<'js>(ctx: &Ctx<'js>, v: &Value<'js>, max: usize) -> (String, bool) {
    let Some(s) = v.as_string().cloned() else { return (String::new(), false) };
    match s.to_cstring() {
        Ok(c) => {
            let bytes = unsafe { std::slice::from_raw_parts(c.as_ptr() as *const u8, c.len()) };
            let mut end = bytes.len().min(max);
            while end < bytes.len() && end > 0 && (bytes[end] & 0xC0) == 0x80 {
                end -= 1;
            }
            let text = replace_surrogates(&bytes[..end]).unwrap_or_else(|| "<too large to print>".into());
            (text, end < bytes.len())
        }
        Err(_) => {
            let _ = ctx.catch();
            ("<too large to print>".into(), true)
        }
    }
}

// ---- encode -----------------------------------------------------------------

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

/// Append `v` as tagged JSON to `out`, which must not grow past `limit` bytes.
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

/// Path segment for an object key in error messages (long keys are cut).
fn key_seg(key: &str) -> String {
    let ident = !key.is_empty()
        && key.chars().enumerate().all(|(i, c)| c == '_' || c == '$' || c.is_ascii_alphabetic() || (i > 0 && c.is_ascii_digit()));
    if ident && key.len() <= 64 {
        format!(".{key}")
    } else {
        let short: String = key.chars().take(64).collect();
        let more = if short.len() < key.len() { "…" } else { "" };
        format!("[{}{more}]", serde_json::to_string(&short).unwrap_or_default())
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

/// An array's length. rquickjs's `Array::len` asserts the length is an int,
/// which fails (a panic, so a trap) from 2^31 up, where QuickJS stores it as a
/// double.
pub fn length(ctx: &Ctx<'_>, arr: &Array<'_>) -> usize {
    match arr.as_object().get::<_, Value>("length") {
        Ok(v) => v.as_number().unwrap_or(0.0).max(0.0) as usize,
        Err(_) => {
            let _ = ctx.catch();
            0
        }
    }
}

fn raw_ptr(v: &Value<'_>) -> usize {
    unsafe { qjs::JS_VALUE_GET_PTR(v.as_raw()) as usize }
}

const HEX: &[u8; 16] = b"0123456789abcdef";

impl<'a, 'js> Encoder<'a, 'js> {
    fn too_large(&self) -> EncodeError {
        EncodeError::new(format!("is larger than the {} byte limit", self.limit))
    }

    /// The only way bytes reach `out`: limit-checked, and grown fallibly so a
    /// full heap is an error instead of a trap.
    fn put(&mut self, b: &[u8]) -> EResult {
        if self.out.len() + b.len() > self.limit {
            return Err(self.too_large());
        }
        self.out.try_reserve(b.len()).map_err(|_| EncodeError::new("is too large for the sandbox's memory"))?;
        self.out.extend_from_slice(b);
        Ok(())
    }

    /// `s` as a JSON string literal.
    fn json_str(&mut self, s: &str) -> EResult {
        // Escaping only grows it, so this fails fast on oversized strings.
        if self.out.len() + s.len() + 2 > self.limit {
            return Err(self.too_large());
        }
        let b = s.as_bytes();
        self.put(b"\"")?;
        let mut start = 0;
        for (i, &c) in b.iter().enumerate() {
            let short: &[u8] = match c {
                b'"' => b"\\\"",
                b'\\' => b"\\\\",
                b'\n' => b"\\n",
                b'\r' => b"\\r",
                b'\t' => b"\\t",
                0x08 => b"\\b",
                0x0c => b"\\f",
                0..=0x1f => b"",
                _ => continue,
            };
            self.put(&b[start..i])?;
            if short.is_empty() {
                self.put(&[b'\\', b'u', b'0', b'0', HEX[(c >> 4) as usize], HEX[(c & 0xf) as usize]])?;
            } else {
                self.put(short)?;
            }
            start = i + 1;
        }
        self.put(&b[start..])?;
        self.put(b"\"")
    }

    /// A JS string value as a JSON string literal, without copying it first.
    fn js_str(&mut self, v: &Value<'js>) -> EResult {
        match utf8(self.ctx, v) {
            Some(Ok(s)) => self.json_str(s.as_str()),
            Some(Err(e)) => Err(EncodeError::new(e.describe())),
            None => self.put(b"null"),
        }
    }

    fn number(&mut self, f: f64) -> EResult {
        if f.is_nan() {
            self.put(br#"{"$t":"n","v":"NaN"}"#)
        } else if f.is_infinite() {
            self.put(if f > 0.0 { br#"{"$t":"n","v":"Infinity"}"# } else { br#"{"$t":"n","v":"-Infinity"}"# })
        } else if f == 0.0 && f.is_sign_negative() {
            self.put(br#"{"$t":"n","v":"-0"}"#)
        } else if f.fract() == 0.0 && f.abs() < 9_007_199_254_740_992.0 {
            self.put((f as i64).to_string().as_bytes())
        } else {
            self.put(serde_json::to_string(&f).unwrap_or_else(|_| "null".into()).as_bytes())
        }
    }

    fn tagged(&mut self, tag: &str) -> EResult {
        self.put(b"{\"$t\":\"")?;
        self.put(tag.as_bytes())?;
        self.put(b"\"")
    }

    fn value(&mut self, v: &Value<'js>) -> EResult {
        if self.out.len() > self.limit {
            return Err(self.too_large());
        }
        match v.type_of() {
            Type::Undefined | Type::Uninitialized => self.put(br#"{"$t":"u"}"#)?,
            Type::Null => self.put(b"null")?,
            Type::Bool => self.put(if v.as_bool() == Some(true) { b"true" } else { b"false" })?,
            Type::Int => self.put(v.as_int().unwrap_or(0).to_string().as_bytes())?,
            Type::Float => self.number(v.as_float().unwrap_or(f64::NAN))?,
            Type::String => self.js_str(v)?,
            Type::BigInt => {
                let s: rquickjs::Coerced<rquickjs::String> = rquickjs::FromJs::from_js(self.ctx, v.clone()).map_err(|_| {
                    let _ = self.ctx.catch();
                    EncodeError::new("is an unreadable BigInt")
                })?;
                self.tagged("bi")?;
                self.put(b",\"v\":")?;
                self.js_str(&s.0.into_value())?;
                self.put(b"}")?;
            }
            Type::Symbol => return Err(EncodeError::new("is a Symbol, which cannot cross the sandbox boundary")),
            Type::Function | Type::Constructor => {
                return Err(EncodeError::new("is a function, which cannot cross the sandbox boundary"))
            }
            Type::Promise => {
                return Err(EncodeError::new("is a Promise, which cannot cross the sandbox boundary (missing `await`?)"))
            }
            Type::Array => self.array(v)?,
            Type::Exception | Type::Object | Type::Proxy => self.object(v)?,
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
        let Some(arr) = v.as_array() else { return self.object(v) };
        self.enter(v)?;
        self.put(b"[")?;
        for i in 0..length(self.ctx, arr) {
            if i > 0 {
                self.put(b",")?;
            }
            let item: Value = arr.get(i).map_err(|_| {
                let _ = self.ctx.catch();
                EncodeError::new("could not be read").at(format!("[{i}]"))
            })?;
            self.value(&item).map_err(|e| e.at(format!("[{i}]")))?;
        }
        self.put(b"]")?;
        self.stack.pop();
        Ok(())
    }

    /// Map entries (`pair`) or Set values, as returned by the prelude hooks.
    fn list(&mut self, items: Array<'js>, pair: bool) -> EResult {
        self.put(b"[")?;
        for i in 0..length(self.ctx, &items) {
            if i > 0 {
                self.put(b",")?;
            }
            let item: Value = items.get(i).map_err(|_| {
                let _ = self.ctx.catch();
                EncodeError::new("could not be read")
            })?;
            if pair {
                let entry = || EncodeError::new("has an entry that is not a [key, value] pair").at(format!(".<entry {i}>"));
                let kv = item.as_array().ok_or_else(entry)?;
                let (k, v): (Value, Value) = match (kv.get(0), kv.get(1)) {
                    (Ok(k), Ok(v)) => (k, v),
                    _ => {
                        let _ = self.ctx.catch();
                        return Err(entry());
                    }
                };
                self.put(b"[")?;
                self.value(&k).map_err(|e| e.at(format!(".<key {i}>")))?;
                self.put(b",")?;
                self.value(&v).map_err(|e| e.at(format!(".<value {i}>")))?;
                self.put(b"]")?;
            } else {
                self.value(&item).map_err(|e| e.at(format!(".<item {i}>")))?;
            }
        }
        self.put(b"]")
    }

    /// A property as a value, or `None` if reading it throws.
    fn prop(&self, obj: &Object<'js>, key: &str) -> Option<Value<'js>> {
        match obj.get::<_, Value>(key) {
            Ok(v) => Some(v),
            Err(_) => {
                let _ = self.ctx.catch();
                None
            }
        }
    }

    fn error(&mut self, obj: &Object<'js>) -> EResult {
        let text = |me: &Self, k: &str| me.prop(obj, k).filter(Value::is_string);
        self.tagged("e")?;
        self.put(b",\"name\":")?;
        match text(self, "name") {
            Some(n) => self.js_str(&n).map_err(|e| e.at(".name".into()))?,
            None => self.put(b"\"Error\"")?,
        }
        self.put(b",\"message\":")?;
        match text(self, "message") {
            Some(m) => self.js_str(&m).map_err(|e| e.at(".message".into()))?,
            None => self.put(b"\"\"")?,
        }
        if let Some(stack) = text(self, "stack") {
            self.put(b",\"stack\":")?;
            self.js_str(&stack).map_err(|e| e.at(".stack".into()))?;
        }
        if let Some(code) = self.prop(obj, "code").filter(|c| !c.is_undefined()) {
            self.put(b",\"code\":")?;
            self.value(&code).map_err(|e| e.at(".code".into()))?;
        }
        if let Some(cause) = self.prop(obj, "cause").filter(|c| !c.is_undefined()) {
            self.enter(obj.as_value())?;
            self.put(b",\"cause\":")?;
            self.value(&cause).map_err(|e| e.at(".cause".into()))?;
            self.stack.pop();
        }
        self.put(b"}")
    }

    fn bytes(&mut self, kind: &str, data: &[u8]) -> EResult {
        self.tagged("b")?;
        if kind != "Uint8Array" {
            self.put(b",\"k\":")?;
            self.json_str(kind)?;
        }
        self.put(b",\"v\":\"")?;
        let n = data.len().div_ceil(3) * 4;
        if self.out.len() + n + 2 > self.limit {
            return Err(self.too_large());
        }
        self.out.try_reserve(n).map_err(|_| EncodeError::new("is too large for the sandbox's memory"))?;
        let start = self.out.len();
        self.out.resize(start + n, 0);
        let written = base64::engine::general_purpose::STANDARD.encode_slice(data, &mut self.out[start..]).unwrap_or(0);
        self.out.truncate(start + written);
        self.put(b"\"}")
    }

    fn object(&mut self, v: &Value<'js>) -> EResult {
        let raw = v.as_raw();
        let ctx_ptr = self.ctx.as_raw().as_ptr();
        let Some(obj) = v.as_object() else { return Err(EncodeError::new("has an unsupported type")) };
        unsafe {
            if qjs::JS_IsDate(raw) {
                let get_time: Function = obj.get("getTime").map_err(|_| {
                    let _ = self.ctx.catch();
                    EncodeError::new("is a broken Date")
                })?;
                let t: f64 = get_time.call((rquickjs::function::This(v.clone()),)).map_err(|_| {
                    let _ = self.ctx.catch();
                    EncodeError::new("is a broken Date")
                })?;
                self.tagged("d")?;
                self.put(b",\"v\":")?;
                if t.is_finite() {
                    self.put((t as i64).to_string().as_bytes())?;
                } else {
                    self.put(b"null")?;
                }
                return self.put(b"}");
            }
            if qjs::JS_IsError(raw) {
                return self.error(obj);
            }
            if qjs::JS_IsRegExp(raw) {
                let src = self.prop(obj, "source").filter(Value::is_string);
                let flags = self.prop(obj, "flags").filter(Value::is_string);
                self.tagged("re")?;
                self.put(b",\"src\":")?;
                match src {
                    Some(s) => self.js_str(&s)?,
                    None => self.put(b"\"\"")?,
                }
                self.put(b",\"flags\":")?;
                match flags {
                    Some(f) => self.js_str(&f)?,
                    None => self.put(b"\"\"")?,
                }
                return self.put(b"}");
            }
            if qjs::JS_IsMap(raw) || qjs::JS_IsSet(raw) {
                let is_map = qjs::JS_IsMap(raw);
                self.enter(v)?;
                let f = if is_map { &self.h.entries } else { &self.h.values };
                let items: Array = f.call((v.clone(),)).map_err(|_| {
                    let _ = self.ctx.catch();
                    EncodeError::new("could not be iterated")
                })?;
                self.tagged(if is_map { "m" } else { "s" })?;
                self.put(b",\"v\":")?;
                self.list(items, is_map)?;
                self.put(b"}")?;
                self.stack.pop();
                return Ok(());
            }
            if qjs::JS_IsArrayBuffer(raw) {
                let mut size = 0usize;
                let ptr = qjs::JS_GetArrayBuffer(ctx_ptr, &mut size as *mut usize as *mut _, raw);
                let data = if ptr.is_null() { &[][..] } else { std::slice::from_raw_parts(ptr, size) };
                return self.bytes("ArrayBuffer", data);
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
                    TypedArray::from_value(v.clone())
                } else {
                    self.h.view_bytes.call((v.clone(),))
                }
                .map_err(|_| {
                    let _ = self.ctx.catch();
                    EncodeError::new("is a detached buffer")
                })?;
                return self.bytes(kind, bytes_of(&view));
            }
            if qjs::JS_IsPromise(raw) {
                return Err(EncodeError::new("is a Promise, which cannot cross the sandbox boundary (missing `await`?)"));
            }
        }

        if let Some(replacement) = self.encoded_as(obj)? {
            self.enter(v)?;
            self.value(&replacement)?;
            self.stack.pop();
            return Ok(());
        }

        self.enter(v)?;
        // An own `$t` key would read as a tag, so such objects are wrapped.
        let mut escaped = false;
        for key in obj.keys::<Atom>() {
            let key = self.key(key)?;
            if key.as_str() == "$t" {
                escaped = true;
                break;
            }
        }
        if escaped {
            self.put(br#"{"$t":"o","v":"#)?;
        }
        self.put(b"{")?;
        let mut first = true;
        for key in obj.keys::<Atom>() {
            let atom = key.map_err(|_| {
                let _ = self.ctx.catch();
                EncodeError::new("has an unreadable key")
            })?;
            let name = self.key(Ok(atom.clone()))?;
            let item: Value = obj.get(atom).map_err(|_| {
                let _ = self.ctx.catch();
                EncodeError::new("could not be read").at(key_seg(name.as_str()))
            })?;
            if !first {
                self.put(b",")?;
            }
            first = false;
            self.json_str(name.as_str()).map_err(|e| e.at(key_seg(name.as_str())))?;
            self.put(b":")?;
            self.value(&item).map_err(|e| e.at(key_seg(name.as_str())))?;
        }
        self.put(b"}")?;
        if escaped {
            self.put(b"}")?;
        }
        self.stack.pop();
        Ok(())
    }

    /// What a built-in with an encode method (URL, URLSearchParams) sends instead.
    fn encoded_as(&self, obj: &Object<'js>) -> Result<Option<Value<'js>>, EncodeError> {
        let Some(f) = self.prop_value(obj, self.h.encode_as.clone()).and_then(Value::into_function) else { return Ok(None) };
        f.call((rquickjs::function::This(obj.clone()),)).map(Some).map_err(|_| {
            let _ = self.ctx.catch();
            EncodeError::new("could not be converted")
        })
    }

    fn prop_value(&self, obj: &Object<'js>, key: Value<'js>) -> Option<Value<'js>> {
        match obj.get::<_, Value>(key) {
            Ok(v) => Some(v),
            Err(_) => {
                let _ = self.ctx.catch();
                None
            }
        }
    }

    fn key(&self, key: rquickjs::Result<Atom<'js>>) -> Result<Utf8<'js>, EncodeError> {
        let unreadable = || EncodeError::new("has an unreadable key");
        let atom = key.map_err(|_| {
            let _ = self.ctx.catch();
            unreadable()
        })?;
        let s = atom.to_js_string().map_err(|_| {
            let _ = self.ctx.catch();
            unreadable()
        })?;
        match utf8(self.ctx, &s.into_value()) {
            Some(Ok(u)) => Ok(u),
            Some(Err(StrError::NotUtf8)) => Err(EncodeError::new("has a key that is not a valid string")),
            _ => Err(unreadable()),
        }
    }
}

// ---- decode -------------------------------------------------------------------

/// Build a JS value from tagged JSON text. Values are created as the text is
/// read, so memory use is the JS value itself (bounded by QuickJS's limit,
/// which throws a catchable error) rather than a parse tree several times the
/// size of the input. Properties are defined, not assigned, so keys like
/// `__proto__` and setters on built-in prototypes don't change the result.
pub fn decode<'js>(ctx: &Ctx<'js>, json: &str, h: &Helpers<'js>) -> Result<Value<'js>, String> {
    let mut de = serde_json::Deserializer::from_str(json);
    let seed = Seed { ctx, h, plain: false };
    let v = seed.deserialize(&mut de).map_err(|e| {
        let _ = ctx.catch();
        e.to_string()
    })?;
    de.end().map_err(|e| e.to_string())?;
    Ok(v)
}

#[derive(Clone, Copy)]
struct Seed<'a, 'js> {
    ctx: &'a Ctx<'js>,
    h: &'a Helpers<'js>,
    /// Read an object's keys literally, without looking for a tag (the
    /// payload of an `o` tag).
    plain: bool,
}

impl<'a, 'js> Seed<'a, 'js> {
    fn child(self) -> Self {
        Seed { plain: false, ..self }
    }
}

/// Turn a failed QuickJS operation (usually out of memory) into a decode
/// error carrying the exception's message, and clear the exception.
fn js<E: de::Error>(ctx: &Ctx<'_>) -> E {
    let exc = ctx.catch();
    let msg = exc.as_object().and_then(|o| o.get::<_, String>("message").ok());
    E::custom(msg.unwrap_or_else(|| "out of memory".into()))
}

fn define<'js, E: de::Error>(ctx: &Ctx<'js>, obj: &Object<'js>, key: impl rquickjs::IntoAtom<'js>, v: Value<'js>) -> Result<(), E> {
    obj.prop(key, Property::from(v).writable().enumerable().configurable()).map_err(|_| js(ctx))
}

impl<'de, 'a, 'js> DeserializeSeed<'de> for Seed<'a, 'js> {
    type Value = Value<'js>;

    fn deserialize<D: de::Deserializer<'de>>(self, d: D) -> Result<Value<'js>, D::Error> {
        d.deserialize_any(self)
    }
}

impl<'de, 'a, 'js> Visitor<'de> for Seed<'a, 'js> {
    type Value = Value<'js>;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a tagged JSON value")
    }

    fn visit_unit<E: de::Error>(self) -> Result<Value<'js>, E> {
        Ok(Value::new_null(self.ctx.clone()))
    }

    fn visit_bool<E: de::Error>(self, b: bool) -> Result<Value<'js>, E> {
        Ok(Value::new_bool(self.ctx.clone(), b))
    }

    fn visit_i64<E: de::Error>(self, i: i64) -> Result<Value<'js>, E> {
        Ok(match i32::try_from(i) {
            Ok(n) => Value::new_int(self.ctx.clone(), n),
            Err(_) => Value::new_number(self.ctx.clone(), i as f64),
        })
    }

    fn visit_u64<E: de::Error>(self, u: u64) -> Result<Value<'js>, E> {
        Ok(match i32::try_from(u) {
            Ok(n) => Value::new_int(self.ctx.clone(), n),
            Err(_) => Value::new_number(self.ctx.clone(), u as f64),
        })
    }

    fn visit_f64<E: de::Error>(self, f: f64) -> Result<Value<'js>, E> {
        Ok(Value::new_number(self.ctx.clone(), f))
    }

    fn visit_str<E: de::Error>(self, s: &str) -> Result<Value<'js>, E> {
        Ok(rquickjs::String::from_str(self.ctx.clone(), s).map_err(|_| js(self.ctx))?.into_value())
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value<'js>, A::Error> {
        let arr = Array::new(self.ctx.clone()).map_err(|_| js(self.ctx))?;
        let mut i = 0u32;
        while let Some(item) = seq.next_element_seed(self.child())? {
            define(self.ctx, &arr, i, item)?;
            i += 1;
        }
        Ok(arr.into_value())
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value<'js>, A::Error> {
        let obj = Object::new(self.ctx.clone()).map_err(|_| js(self.ctx))?;
        let Some(first) = map.next_key::<Key>()? else { return Ok(obj.into_value()) };
        if !self.plain && first.0 == "$t" {
            let tag: Key = map.next_value()?;
            return tagged(self, &tag.0, map);
        }
        let v = map.next_value_seed(self.child())?;
        define(self.ctx, &obj, &*first.0, v)?;
        while let Some(k) = map.next_key::<Key>()? {
            let v = map.next_value_seed(self.child())?;
            define(self.ctx, &obj, &*k.0, v)?;
        }
        Ok(obj.into_value())
    }
}

/// A string that borrows from the input when it has no escapes.
struct Key<'de>(Cow<'de, str>);

impl<'de> de::Deserialize<'de> for Key<'de> {
    fn deserialize<D: de::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Key<'de>;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a string")
            }
            fn visit_borrowed_str<E: de::Error>(self, s: &'de str) -> Result<Key<'de>, E> {
                Ok(Key(Cow::Borrowed(s)))
            }
            fn visit_str<E: de::Error>(self, s: &str) -> Result<Key<'de>, E> {
                Ok(Key(Cow::Owned(s.to_owned())))
            }
        }
        d.deserialize_str(V)
    }
}

/// The rest of a tagged object, after `"$t": tag`.
fn tagged<'de, 'a, 'js, A: MapAccess<'de>>(seed: Seed<'a, 'js>, tag: &str, mut map: A) -> Result<Value<'js>, A::Error> {
    let ctx = seed.ctx;
    let h = seed.h;
    let mut v: Option<Value<'js>> = None;
    let mut bytes: Option<Vec<u8>> = None;
    let mut fields: Vec<(String, Value<'js>)> = Vec::new();
    while let Some(k) = map.next_key::<Key>()? {
        match (tag, &*k.0) {
            ("b", "v") => {
                let b64: Key = map.next_value()?;
                let mut buf = Vec::new();
                let n = base64::decoded_len_estimate(b64.0.len());
                buf.try_reserve(n).map_err(|_| de::Error::custom("the value is too large for the sandbox's memory"))?;
                base64::engine::general_purpose::STANDARD
                    .decode_vec(b64.0.as_bytes(), &mut buf)
                    .map_err(|_| de::Error::custom("bytes are not valid base64"))?;
                bytes = Some(buf);
            }
            ("o", "v") => v = Some(map.next_value_seed(Seed { plain: true, ..seed })?),
            (_, "v") => v = Some(map.next_value_seed(seed.child())?),
            (_, "k" | "src" | "flags" | "name" | "message" | "stack" | "code" | "cause" | "data") => {
                fields.push((k.0.into_owned(), map.next_value_seed(seed.child())?));
            }
            _ => {
                map.next_value::<IgnoredAny>()?;
            }
        }
    }
    let field = |name: &str| fields.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone());
    // String fields go to the prelude as JS values, so they're never copied into Rust.
    let text = |name: &str, default: &str| -> Result<Value<'js>, A::Error> {
        match field(name).filter(Value::is_string) {
            Some(v) => Ok(v),
            None => Ok(rquickjs::String::from_str(ctx.clone(), default).map_err(|_| js(ctx))?.into_value()),
        }
    };
    let undefined = || Value::new_undefined(ctx.clone());
    Ok(match tag {
        "u" => undefined(),
        "n" => {
            let which = v.as_ref().and_then(|v| utf8(ctx, v)?.ok().map(|s| s.as_str().to_owned()));
            Value::new_float(
                ctx.clone(),
                match which.as_deref() {
                    Some("Infinity") => f64::INFINITY,
                    Some("-Infinity") => f64::NEG_INFINITY,
                    Some("-0") => -0.0,
                    _ => f64::NAN,
                },
            )
        }
        "bi" => h.mk_bigint.call((v.unwrap_or_else(undefined),)).map_err(|_| js(ctx))?,
        "d" => unsafe {
            let ms = v.and_then(|v| v.as_number()).unwrap_or(f64::NAN);
            Value::from_raw(ctx.clone(), qjs::JS_NewDate(ctx.as_raw().as_ptr(), ms))
        },
        "b" => {
            let kind = field("k").and_then(|k| utf8(ctx, &k)?.ok().map(|s| s.as_str().to_owned()));
            let u8s = TypedArray::<u8>::new_copy(ctx.clone(), bytes.unwrap_or_default()).map_err(|_| js(ctx))?;
            match kind.as_deref().unwrap_or("Uint8Array") {
                "Uint8Array" => u8s.into_value(),
                "ArrayBuffer" => u8s.arraybuffer().map_err(|_| js(ctx))?.into_value(),
                other => h.mk_view.call((other, u8s)).map_err(|_| js(ctx))?,
            }
        }
        "m" | "s" => {
            let arr = match v {
                Some(a) if a.is_array() => a,
                _ => Array::new(ctx.clone()).map_err(|_| js(ctx))?.into_value(),
            };
            let f = if tag == "m" { &h.mk_map } else { &h.mk_set };
            f.call((arr,)).map_err(|_| js(ctx))?
        }
        "re" => h.mk_regexp.call((text("src", "")?, text("flags", "")?)).map_err(|_| js(ctx))?,
        "e" => {
            let extra = Object::new(ctx.clone()).map_err(|_| js(ctx))?;
            for k in ["code", "cause", "data"] {
                if let Some(x) = field(k) {
                    define(ctx, &extra, k, x)?;
                }
            }
            let stack = field("stack").unwrap_or_else(undefined);
            h.mk_error.call((text("name", "Error")?, text("message", "")?, stack, extra)).map_err(|_| js(ctx))?
        }
        "o" => match v {
            Some(o) if o.is_object() => o,
            _ => Object::new(ctx.clone()).map_err(|_| js(ctx))?.into_value(),
        },
        _ => return Err(de::Error::custom(format!("unknown value tag {tag:?}"))),
    })
}
