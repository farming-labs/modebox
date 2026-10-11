//! TypeScript declarations for a [`Toolbox`], generated from tool JSON Schemas.
//! This is what the model reads to learn the API it can call from code.

use std::collections::BTreeMap;

use serde_json::Value as Json;

use crate::tools::{Tool, Toolbox};

const MAX_DEPTH: usize = 8;

/// Render `declare` statements for every tool, grouped into namespaces.
pub fn declarations(tools: &Toolbox) -> String {
    let mut root = Node::default();
    let mut internal = false;
    for tool in tools.iter() {
        // `$`-names are host internals behind built-ins (`$fetch` backs `fetch`).
        if tool.name.starts_with('$') {
            internal |= tool.name == "$fetch";
            continue;
        }
        let mut node = &mut root;
        let parts: Vec<&str> = tool.name.split('.').collect();
        for seg in &parts[..parts.len() - 1] {
            node = node.children.entry((*seg).to_owned()).or_default();
        }
        node.fns.insert(parts[parts.len() - 1].to_owned(), tool.clone());
    }

    let mut out = String::new();
    if internal {
        out.push_str("/** Standard fetch(); requests go through the host, which allows some hosts and may ask about others. */\n");
        out.push_str("declare function fetch(input: string | URL | Request, init?: RequestInit): Promise<Response>;\n");
    }
    for (name, tool) in &root.fns {
        doc(&mut out, tool.description.as_deref(), "");
        out.push_str(&format!("declare function {name}{};\n", signature(tool)));
    }
    for (name, child) in &root.children {
        out.push_str(&format!("declare const {name}: "));
        render_ns(&mut out, child, 0);
        out.push_str(";\n");
    }
    out
}

#[derive(Default)]
struct Node {
    fns: BTreeMap<String, Tool>,
    children: BTreeMap<String, Node>,
}

fn render_ns(out: &mut String, node: &Node, depth: usize) {
    let pad = "  ".repeat(depth + 1);
    out.push_str("{\n");
    for (name, tool) in &node.fns {
        doc(out, tool.description.as_deref(), &pad);
        out.push_str(&format!("{pad}{name}{};\n", signature(tool)));
    }
    for (name, child) in &node.children {
        out.push_str(&format!("{pad}{name}: "));
        render_ns(out, child, depth + 1);
        out.push_str(";\n");
    }
    out.push_str(&"  ".repeat(depth));
    out.push('}');
}

fn doc(out: &mut String, text: Option<&str>, pad: &str) {
    let Some(text) = text.map(str::trim).filter(|t| !t.is_empty()) else { return };
    let text = text.replace("*/", "*\\/");
    if !text.contains('\n') && text.len() < 100 {
        out.push_str(&format!("{pad}/** {text} */\n"));
    } else {
        out.push_str(&format!("{pad}/**\n"));
        for line in text.lines() {
            out.push_str(&format!("{pad} * {line}\n"));
        }
        out.push_str(&format!("{pad} */\n"));
    }
}

fn signature(tool: &Tool) -> String {
    let ret = tool.output_schema.as_ref().map(|s| ts_type(s, s, 0)).unwrap_or_else(|| "unknown".into());
    match &tool.input_schema {
        Some(schema) => {
            let ty = ts_type(schema, schema, 0);
            let optional = schema.get("required").and_then(Json::as_array).is_none_or(|r| r.is_empty())
                && schema.get("type").and_then(Json::as_str) == Some("object");
            format!("(input{}: {ty}): Promise<{ret}>", if optional { "?" } else { "" })
        }
        None => format!("(...args: unknown[]): Promise<{ret}>"),
    }
}

fn is_ident(s: &str) -> bool {
    let mut c = s.chars();
    matches!(c.next(), Some(ch) if ch == '_' || ch == '$' || ch.is_ascii_alphabetic())
        && c.all(|ch| ch == '_' || ch == '$' || ch.is_ascii_alphanumeric())
}

fn resolve<'a>(root: &'a Json, reference: &str) -> Option<&'a Json> {
    let path = reference.strip_prefix("#/")?;
    let mut node = root;
    for seg in path.split('/') {
        node = node.get(seg.replace("~1", "/").replace("~0", "~"))?;
    }
    Some(node)
}

/// JSON Schema → TypeScript type expression.
pub(crate) fn ts_type(schema: &Json, root: &Json, depth: usize) -> String {
    if depth > MAX_DEPTH {
        return "unknown".into();
    }
    let Some(obj) = schema.as_object() else {
        return if schema == &Json::Bool(false) { "never".into() } else { "unknown".into() };
    };
    // Escape hatch for types JSON Schema can't express (Date, Uint8Array, ...).
    if let Some(t) = obj.get("tsType").and_then(Json::as_str) {
        return t.to_owned();
    }
    if let Some(r) = obj.get("$ref").and_then(Json::as_str) {
        return resolve(root, r).map(|s| ts_type(s, root, depth + 1)).unwrap_or_else(|| "unknown".into());
    }
    if let Some(c) = obj.get("const") {
        return serde_json::to_string(c).unwrap();
    }
    if let Some(Json::Array(vals)) = obj.get("enum") {
        return vals.iter().map(|v| serde_json::to_string(v).unwrap()).collect::<Vec<_>>().join(" | ");
    }
    for (key, sep) in [("anyOf", " | "), ("oneOf", " | "), ("allOf", " & ")] {
        if let Some(Json::Array(parts)) = obj.get(key) {
            let mut types: Vec<String> = parts.iter().map(|p| ts_type(p, root, depth + 1)).collect();
            types.dedup();
            return if types.len() == 1 { types.remove(0) } else { format!("({})", types.join(sep)) };
        }
    }
    match obj.get("type") {
        Some(Json::Array(types)) => {
            let parts: Vec<String> = types
                .iter()
                .filter_map(Json::as_str)
                .map(|t| {
                    let mut single = obj.clone();
                    single.insert("type".into(), Json::String(t.into()));
                    ts_type(&Json::Object(single), root, depth + 1)
                })
                .collect();
            parts.join(" | ")
        }
        Some(Json::String(t)) => match t.as_str() {
            "string" => "string".into(),
            "number" | "integer" => "number".into(),
            "boolean" => "boolean".into(),
            "null" => "null".into(),
            "array" => {
                let item = obj.get("items").map(|i| ts_type(i, root, depth + 1)).unwrap_or_else(|| "unknown".into());
                if item.contains(' ') { format!("Array<{item}>") } else { format!("{item}[]") }
            }
            "object" => object_type(obj, root, depth),
            _ => "unknown".into(),
        },
        _ if obj.contains_key("properties") => object_type(obj, root, depth),
        _ => "unknown".into(),
    }
}

fn object_type(obj: &serde_json::Map<String, Json>, root: &Json, depth: usize) -> String {
    let required: Vec<&str> =
        obj.get("required").and_then(Json::as_array).map(|r| r.iter().filter_map(Json::as_str).collect()).unwrap_or_default();
    let props = obj.get("properties").and_then(Json::as_object);
    let mut fields = Vec::new();
    if let Some(props) = props {
        for (name, schema) in props {
            let key = if is_ident(name) { name.clone() } else { serde_json::to_string(name).unwrap() };
            let opt = if required.contains(&name.as_str()) { "" } else { "?" };
            let ty = ts_type(schema, root, depth + 1);
            let desc = schema.get("description").and_then(Json::as_str).map(|d| d.replace("*/", "*\\/"));
            match desc {
                Some(d) if !d.trim().is_empty() => {
                    fields.push(format!("/** {} */ {key}{opt}: {ty}", d.trim().replace('\n', " ")))
                }
                _ => fields.push(format!("{key}{opt}: {ty}")),
            }
        }
    }
    match obj.get("additionalProperties") {
        Some(Json::Bool(false)) | None if !fields.is_empty() => {}
        Some(extra @ Json::Object(_)) => fields.push(format!("[key: string]: {}", ts_type(extra, root, depth + 1))),
        _ if fields.is_empty() => return "Record<string, unknown>".into(),
        _ => {}
    }
    format!("{{ {} }}", fields.join("; "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{Call, Tool};
    use crate::value::Value;
    use serde_json::json;

    #[test]
    fn renders_namespaces_and_schemas() {
        let mut tb = Toolbox::new();
        tb.add(
            Tool::new("github.issues.create", |_c: Call| async { Ok(Value::Null) })
                .description("Open an issue.")
                .input_schema(json!({
                    "type": "object",
                    "properties": {
                        "repo": { "type": "string", "description": "owner/name" },
                        "labels": { "type": "array", "items": { "type": "string" } },
                        "state": { "enum": ["open", "closed"] },
                        "assignee": { "$ref": "#/$defs/user" }
                    },
                    "required": ["repo"],
                    "$defs": { "user": { "type": ["string", "null"] } }
                }))
                .output_schema(json!({ "type": "object", "properties": { "number": { "type": "integer" } }, "required": ["number"] })),
        );
        tb.add(Tool::new("sh", |_c: Call| async { Ok(Value::Null) }));
        let d = declarations(&tb);
        assert!(d.contains("declare function sh(...args: unknown[]): Promise<unknown>;"), "{d}");
        assert!(d.contains("declare const github: {"), "{d}");
        assert!(d.contains("issues: {"), "{d}");
        assert!(d.contains("/** Open an issue. */"), "{d}");
        assert!(
            d.contains(r#"create(input: { /** owner/name */ repo: string; labels?: string[]; state?: "open" | "closed"; assignee?: string | null }): Promise<{ number: number }>;"#),
            "{d}"
        );
    }
}
