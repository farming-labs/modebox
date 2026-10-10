//! Node-style value formatting for console output (`util.inspect`-like).
//! Models have seen a lot of Node output, so matching its shape makes logs
//! easy for them to read back.

use rquickjs::{qjs, Array, Ctx, Function, Object, Type, Value};

use crate::codec::Helpers;

const MAX_DEPTH: usize = 2;
const MAX_ITEMS: usize = 100;
const MAX_STRING: usize = 10_000;
const LINE_WIDTH: usize = 72;

/// Format a value. `top` strings print raw (like `console.log("x")`).
pub fn inspect<'js>(ctx: &Ctx<'js>, v: &Value<'js>, h: &Helpers<'js>, top: bool) -> String {
    let mut f = Fmt { ctx, h, seen: Vec::new() };
    let mut s = if top && v.is_string() {
        v.as_string().and_then(|s| s.to_string().ok()).unwrap_or_default()
    } else {
        f.fmt(v, 0)
    };
    if s.len() > MAX_STRING {
        let mut cut = MAX_STRING;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
        s.push_str("… (truncated)");
    }
    s
}

struct Fmt<'a, 'js> {
    ctx: &'a Ctx<'js>,
    h: &'a Helpers<'js>,
    seen: Vec<usize>,
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
    fn js_string(&self, v: &Value<'js>) -> String {
        rquickjs::FromJs::from_js(self.ctx, v.clone())
            .map(|c: rquickjs::Coerced<String>| c.0)
            .unwrap_or_else(|_| "<unprintable>".into())
    }

    fn call_str(&self, obj: &Object<'js>, method: &str) -> Option<String> {
        let f: Function = obj.get(method).ok()?;
        f.call((rquickjs::function::This(obj.clone()),)).ok()
    }

    fn fmt(&mut self, v: &Value<'js>, depth: usize) -> String {
        match v.type_of() {
            Type::Undefined | Type::Uninitialized => "undefined".into(),
            Type::Null => "null".into(),
            Type::Bool => v.as_bool().unwrap().to_string(),
            Type::Int => v.as_int().unwrap().to_string(),
            Type::Float => {
                let f = v.as_float().unwrap();
                if f == 0.0 && f.is_sign_negative() { "-0".into() } else { self.js_string(v) }
            }
            Type::String => quote(&v.as_string().and_then(|s| s.to_string().ok()).unwrap_or_default()),
            Type::BigInt => format!("{}n", self.js_string(v)),
            Type::Symbol => {
                let desc = v
                    .as_symbol()
                    .and_then(|s| s.description().ok())
                    .and_then(|d| d.as_string().and_then(|s| s.to_string().ok()))
                    .unwrap_or_default();
                format!("Symbol({desc})")
            }
            Type::Function | Type::Constructor => {
                let obj = v.as_object().unwrap();
                let name: String = obj.get("name").unwrap_or_default();
                let src = self.call_str(obj, "toString").unwrap_or_default();
                if src.starts_with("class") {
                    format!("[class {}]", if name.is_empty() { "(anonymous)" } else { &name })
                } else if name.is_empty() {
                    "[Function (anonymous)]".into()
                } else {
                    format!("[Function: {name}]")
                }
            }
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
            Type::Exception => self.error(v.as_object().unwrap(), depth),
            Type::Array | Type::Object | Type::Proxy => self.object(v, depth),
            other => format!("[{}]", other.as_str()),
        }
    }

    fn error(&mut self, obj: &Object<'js>, depth: usize) -> String {
        let name: String = obj.get("name").unwrap_or_else(|_| "Error".into());
        let message: String = obj.get("message").unwrap_or_default();
        let head = if message.is_empty() { name } else { format!("{name}: {message}") };
        if depth > 0 {
            return format!("[{head}]");
        }
        match obj.get::<_, Option<String>>("stack").ok().flatten() {
            Some(stack) if !stack.trim().is_empty() => format!("{head}\n{}", stack.trim_end()),
            _ => head,
        }
    }

    fn object(&mut self, v: &Value<'js>, depth: usize) -> String {
        let ptr = unsafe { qjs::JS_VALUE_GET_PTR(v.as_raw()) as usize };
        if self.seen.contains(&ptr) {
            return "[Circular]".into();
        }
        let obj = v.as_object().unwrap();
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
        let ctor: String = self.h.ctor_name.call((v.clone(),)).unwrap_or_default();

        if is_buffer {
            let len: usize = obj.get("byteLength").unwrap_or(0);
            return format!("ArrayBuffer {{ byteLength: {len} }}");
        }
        if depth > MAX_DEPTH {
            return if is_array { "[Array]".into() } else if ctor.is_empty() || ctor == "Object" { "[Object]".into() } else { format!("[{ctor}]") };
        }

        self.seen.push(ptr);
        let out = if is_array || typed {
            let len: usize = obj.get("length").unwrap_or(0);
            let mut items = Vec::new();
            for i in 0..len.min(MAX_ITEMS) {
                let item: Value = obj.get(i as u32).unwrap_or_else(|_| Value::new_undefined(self.ctx.clone()));
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
            let f = if is_map { &self.h.entries } else { &self.h.values };
            let list: Array = f.call((v.clone(),)).unwrap_or_else(|_| Array::new(self.ctx.clone()).unwrap());
            let mut items = Vec::new();
            for i in 0..list.len().min(MAX_ITEMS) {
                let item: Value = list.get(i).unwrap();
                if is_map {
                    let kv = item.as_array().unwrap();
                    let k = self.fmt(&kv.get(0).unwrap(), depth + 1);
                    let val = self.fmt(&kv.get(1).unwrap(), depth + 1);
                    items.push(format!("{k} => {val}"));
                } else {
                    items.push(self.fmt(&item, depth + 1));
                }
            }
            if list.len() > MAX_ITEMS {
                items.push(format!("... {} more items", list.len() - MAX_ITEMS));
            }
            wrap(&format!("{}({}) {{", if is_map { "Map" } else { "Set" }, list.len()), items, "}", depth)
        } else {
            let mut items = Vec::new();
            for (n, k) in obj.keys::<String>().enumerate() {
                let Ok(k) = k else { continue };
                if n >= MAX_ITEMS {
                    items.push("...".into());
                    break;
                }
                let val: Value = obj.get(k.as_str()).unwrap_or_else(|_| Value::new_undefined(self.ctx.clone()));
                items.push(format!("{}: {}", key(&k), self.fmt(&val, depth + 1)));
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
