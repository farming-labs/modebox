//! Node-style value formatting for console output (`util.inspect`-like).
//! Models have seen a lot of Node output, so matching its shape makes logs
//! easy for them to read back.
//!
//! Output is bounded no matter the input: strings are cut before they are
//! copied out of QuickJS, and a budget stops formatting once enough text has
//! been produced, so logging a huge value can't exhaust the guest's memory.

use rquickjs::{qjs, Atom, Ctx, Function, Object, Type, Value};

use crate::codec::{clip, length, Helpers};

const MAX_DEPTH: usize = 2;
const MAX_ITEMS: usize = 100;
/// Longest output for one value; longer output is cut.
pub const MAX_STRING: usize = 10_000;
const LINE_WIDTH: usize = 72;
const TRUNCATED: &str = "… (truncated)";

/// Format a value. `top` strings print raw (like `console.log("x")`).
pub fn inspect<'js>(ctx: &Ctx<'js>, v: &Value<'js>, h: &Helpers<'js>, top: bool) -> String {
    let mut f = Fmt { ctx, h, seen: Vec::new(), budget: MAX_STRING };
    let mut s = if top && v.is_string() {
        let (s, cut) = clip(ctx, v, MAX_STRING);
        if cut { s + TRUNCATED } else { s }
    } else {
        f.fmt(v, 0)
    };
    if s.len() > MAX_STRING {
        let mut cut = MAX_STRING;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
        s.push_str(TRUNCATED);
    }
    s
}

struct Fmt<'a, 'js> {
    ctx: &'a Ctx<'js>,
    h: &'a Helpers<'js>,
    seen: Vec<usize>,
    /// Bytes of leaf text still allowed; at zero, everything left prints as `…`.
    budget: usize,
}

fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        match c {
            '\'' => out.push_str("\\'"),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('\'');
    out
}

fn key(k: &str) -> String {
    let ident = !k.is_empty()
        && k.chars().enumerate().all(|(i, c)| c == '_' || c == '$' || c.is_ascii_alphabetic() || (i > 0 && c.is_ascii_digit()));
    if ident { k.to_owned() } else { quote(k) }
}

fn wrap(open: &str, items: Vec<String>, close: &str, depth: usize) -> String {
    if items.is_empty() {
        return format!("{}{}", open.trim_end(), close.trim_start());
    }
    let total: usize = items.iter().map(|s| s.len() + 2).sum::<usize>() + open.len();
    if total <= LINE_WIDTH && !items.iter().any(|s| s.contains('\n')) {
        format!("{open} {} {close}", items.join(", "))
    } else {
        let pad = "  ".repeat(depth + 1);
        let body: Vec<String> = items.iter().map(|s| format!("{pad}{}", s.replace('\n', &format!("\n{pad}")))).collect();
        format!("{open}\n{}\n{}{close}", body.join(",\n"), "  ".repeat(depth))
    }
}

impl<'a, 'js> Fmt<'a, 'js> {
    /// Text of a JS string value, cut to what the budget allows.
    fn text(&mut self, v: &Value<'js>) -> String {
        let (s, cut) = clip(self.ctx, v, self.budget);
        self.budget = self.budget.saturating_sub(s.len());
        if cut { s + "…" } else { s }
    }

    /// `String(v)`, cut to the budget.
    fn coerced(&mut self, v: &Value<'js>) -> String {
        match rquickjs::FromJs::from_js(self.ctx, v.clone()) {
            Ok(rquickjs::Coerced::<rquickjs::String>(s)) => self.text(&s.into_value()),
            Err(_) => {
                let _ = self.ctx.catch();
                "<unprintable>".into()
            }
        }
    }

    fn get(&self, obj: &Object<'js>, k: &str) -> Value<'js> {
        obj.get(k).unwrap_or_else(|_| {
            let _ = self.ctx.catch();
            Value::new_undefined(self.ctx.clone())
        })
    }

    /// Call a zero-argument method and return its string result.
    fn call_str(&mut self, obj: &Object<'js>, method: &str) -> Option<String> {
        let f: Function = self.get(obj, method).into_function()?;
        match f.call::<_, Value>((rquickjs::function::This(obj.clone()),)) {
            Ok(v) if v.is_string() => Some(self.text(&v)),
            Ok(_) => None,
            Err(_) => {
                let _ = self.ctx.catch();
                None
            }
        }
    }

    fn fmt(&mut self, v: &Value<'js>, depth: usize) -> String {
        if self.budget == 0 {
            return "…".into();
        }
        let out = match v.type_of() {
            Type::Undefined | Type::Uninitialized => "undefined".into(),
            Type::Null => "null".into(),
            Type::Bool => (v.as_bool() == Some(true)).to_string(),
            Type::Int => v.as_int().unwrap_or(0).to_string(),
            Type::Float => {
                let f = v.as_float().unwrap_or(f64::NAN);
                if f == 0.0 && f.is_sign_negative() { "-0".into() } else { self.coerced(v) }
            }
            Type::String => quote(&self.text(v)),
            Type::BigInt => format!("{}n", self.coerced(v)),
            Type::Symbol => {
                let desc = v.as_symbol().and_then(|s| s.description().ok());
                let desc = desc.map(|d| self.text(&d)).unwrap_or_default();
                format!("Symbol({desc})")
            }
            Type::Function | Type::Constructor => match v.as_object() {
                Some(obj) => {
                    let name = self.get(obj, "name");
                    let name = if name.is_string() { self.text(&name) } else { String::new() };
                    let src = self.call_str(obj, "toString").unwrap_or_default();
                    if src.starts_with("class") {
                        format!("[class {}]", if name.is_empty() { "(anonymous)" } else { &name })
                    } else if name.is_empty() {
                        "[Function (anonymous)]".into()
                    } else {
                        format!("[Function: {name}]")
                    }
                }
                None => "[Function]".into(),
            },
            Type::Promise => {
                let raw = v.as_raw();
                let ctx_ptr = self.ctx.as_raw().as_ptr();
                let state = unsafe { qjs::JS_PromiseState(ctx_ptr, raw) };
                match state {
                    qjs::JSPromiseStateEnum_JS_PROMISE_PENDING => "Promise { <pending> }".into(),
                    _ => {
                        let r = unsafe { Value::from_raw(self.ctx.clone(), qjs::JS_PromiseResult(ctx_ptr, raw)) };
                        let inner = self.fmt(&r, depth + 1);
                        if state == qjs::JSPromiseStateEnum_JS_PROMISE_REJECTED {
                            format!("Promise {{ <rejected> {inner} }}")
                        } else {
                            format!("Promise {{ {inner} }}")
                        }
                    }
                }
            }
            Type::Exception | Type::Array | Type::Object | Type::Proxy => self.object(v, depth),
            other => format!("[{}]", other.as_str()),
        };
        // Structure counts against the budget too, so wide values end.
        self.budget = self.budget.saturating_sub(out.len().min(16));
        out
    }

    fn error(&mut self, obj: &Object<'js>, depth: usize) -> String {
        let name = self.get(obj, "name");
        let name = if name.is_string() { self.text(&name) } else { "Error".into() };
        let message = self.get(obj, "message");
        let message = if message.is_string() { self.text(&message) } else { String::new() };
        let head = if message.is_empty() { name } else { format!("{name}: {message}") };
        if depth > 0 {
            return format!("[{head}]");
        }
        let stack = self.get(obj, "stack");
        let stack = if stack.is_string() { self.text(&stack) } else { String::new() };
        if stack.trim().is_empty() { head } else { format!("{head}\n{}", stack.trim_end()) }
    }

    /// Map entries or Set values, from the prelude hooks.
    fn entries(&mut self, v: &Value<'js>, is_map: bool, depth: usize) -> (Vec<String>, usize) {
        let f = if is_map { &self.h.entries } else { &self.h.values };
        let list: rquickjs::Array = match f.call((v.clone(),)) {
            Ok(l) => l,
            Err(_) => {
                let _ = self.ctx.catch();
                return (vec!["<unreadable>".into()], 0);
            }
        };
        let len = length(self.ctx, &list);
        let mut items = Vec::new();
        for i in 0..len.min(MAX_ITEMS) {
            if self.budget == 0 {
                items.push("…".into());
                break;
            }
            let item: Value = list.get(i).unwrap_or_else(|_| {
                let _ = self.ctx.catch();
                Value::new_undefined(self.ctx.clone())
            });
            match (is_map, item.as_array()) {
                (true, Some(kv)) => {
                    let k: Value = kv.get(0).unwrap_or_else(|_| Value::new_undefined(self.ctx.clone()));
                    let val: Value = kv.get(1).unwrap_or_else(|_| Value::new_undefined(self.ctx.clone()));
                    let k = self.fmt(&k, depth + 1);
                    let val = self.fmt(&val, depth + 1);
                    items.push(format!("{k} => {val}"));
                }
                _ => items.push(self.fmt(&item, depth + 1)),
            }
        }
        if len > MAX_ITEMS {
            items.push(format!("... {} more items", len - MAX_ITEMS));
        }
        (items, len)
    }

    fn object(&mut self, v: &Value<'js>, depth: usize) -> String {
        let ptr = unsafe { qjs::JS_VALUE_GET_PTR(v.as_raw()) as usize };
        if self.seen.contains(&ptr) {
            return "[Circular]".into();
        }
        let Some(obj) = v.as_object() else { return "[object]".into() };
        let raw = v.as_raw();
        unsafe {
            if qjs::JS_IsDate(raw) {
                return self.call_str(obj, "toISOString").unwrap_or_else(|| "Invalid Date".into());
            }
            if qjs::JS_IsRegExp(raw) {
                return self.call_str(obj, "toString").unwrap_or_default();
            }
            if qjs::JS_IsError(raw) {
                return self.error(obj, depth);
            }
        }
        let is_array = v.is_array();
        let is_map = unsafe { qjs::JS_IsMap(raw) };
        let is_set = unsafe { qjs::JS_IsSet(raw) };
        let is_buffer = unsafe { qjs::JS_IsArrayBuffer(raw) };
        let typed = unsafe { qjs::JS_GetTypedArrayType(raw) } >= 0;
        let ctor = match self.h.ctor_name.call::<_, Value>((v.clone(),)) {
            Ok(c) if c.is_string() => self.text(&c),
            Ok(_) => String::new(),
            Err(_) => {
                let _ = self.ctx.catch();
                String::new()
            }
        };

        if is_buffer {
            let len = self.get(obj, "byteLength").as_number().unwrap_or(0.0);
            return format!("ArrayBuffer {{ byteLength: {len} }}");
        }
        if depth > MAX_DEPTH {
            return if is_array { "[Array]".into() } else if ctor.is_empty() || ctor == "Object" { "[Object]".into() } else { format!("[{ctor}]") };
        }

        self.seen.push(ptr);
        let out = if is_array || typed {
            let len = self.get(obj, "length").as_number().unwrap_or(0.0).max(0.0) as usize;
            let mut items = Vec::new();
            for i in 0..len.min(MAX_ITEMS) {
                if self.budget == 0 {
                    items.push("…".into());
                    break;
                }
                let item: Value = obj.get(i as u32).unwrap_or_else(|_| {
                    let _ = self.ctx.catch();
                    Value::new_undefined(self.ctx.clone())
                });
                items.push(self.fmt(&item, depth + 1));
            }
            if len > MAX_ITEMS {
                items.push(format!("... {} more item{}", len - MAX_ITEMS, if len - MAX_ITEMS == 1 { "" } else { "s" }));
            }
            if typed {
                wrap(&format!("{ctor}({len}) ["), items, "]", depth)
            } else {
                wrap("[", items, "]", depth)
            }
        } else if is_map || is_set {
            let (items, len) = self.entries(v, is_map, depth);
            wrap(&format!("{}({len}) {{", if is_map { "Map" } else { "Set" }), items, "}", depth)
        } else {
            let mut items = Vec::new();
            for (n, k) in obj.keys::<Atom>().enumerate() {
                if n >= MAX_ITEMS || self.budget == 0 {
                    items.push("...".into());
                    break;
                }
                let Ok(k) = k else {
                    let _ = self.ctx.catch();
                    continue;
                };
                let name = match k.to_js_string() {
                    Ok(s) => self.text(&s.into_value()),
                    Err(_) => {
                        let _ = self.ctx.catch();
                        continue;
                    }
                };
                let val: Value = obj.get(k).unwrap_or_else(|_| {
                    let _ = self.ctx.catch();
                    Value::new_undefined(self.ctx.clone())
                });
                items.push(format!("{}: {}", key(&name), self.fmt(&val, depth + 1)));
            }
            let open = if ctor.is_empty() {
                "[Object: null prototype] {".to_owned()
            } else if ctor == "Object" {
                "{".to_owned()
            } else {
                format!("{ctor} {{")
            };
            wrap(&open, items, "}", depth)
        };
        self.seen.pop();
        out
    }
}
