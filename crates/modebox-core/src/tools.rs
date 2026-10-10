//! Host functions ("tools") the sandbox may call, and the policy around them.
//!
//! Guest code sees each tool as an async function at its dotted path, e.g. a
//! tool named `github.issues.list` is called as `await github.issues.list({...})`.
//! The sandbox has no other authority: no network, files, env or clock beyond
//! what these functions do on its behalf.

use std::collections::BTreeMap;
use std::fmt;
use std::future::Future;
use std::sync::Arc;

use futures::future::BoxFuture;

use crate::value::Value;

/// One invocation of a tool.
#[derive(Clone, Debug)]
pub struct Call {
    pub name: String,
    pub args: Vec<Value>,
    /// Stable id of this call within its run (survives suspend/resume).
    pub id: u32,
    pub run_id: String,
    /// Set when this call is being re-invoked after an interrupt was resolved
    /// with [`crate::Resolution::ResumeWith`].
    pub resume: Option<Value>,
}

impl Call {
    /// First argument, the common "single input object" calling convention.
    pub fn input(&self) -> &Value {
        static UNDEFINED: Value = Value::Undefined;
        self.args.first().unwrap_or(&UNDEFINED)
    }

    /// Pause the whole run and hand a question back to the application (for
    /// example "this needs approval" or "user must sign in"). The run returns
    /// [`crate::Outcome::Suspended`]; when resumed, this call is re-invoked with
    /// [`Call::resume`] set, or settled directly, depending on the resolution.
    pub fn interrupt(&self, kind: impl Into<String>, payload: impl Into<Value>) -> ToolError {
        ToolError { interrupt: Some((kind.into(), payload.into())), ..ToolError::new("interrupted") }
    }
}

/// A tool failure. Becomes a rejected promise (an `Error`) inside the sandbox.
#[derive(Clone, Debug, Default)]
pub struct ToolError {
    pub name: String,
    pub message: String,
    pub code: Option<String>,
    pub data: Option<Value>,
    pub(crate) interrupt: Option<(String, Value)>,
}

impl ToolError {
    pub fn new(message: impl Into<String>) -> Self {
        ToolError { name: "Error".into(), message: message.into(), ..Default::default() }
    }

    pub fn with_code(mut self, code: impl Into<String>) -> Self {
        self.code = Some(code.into());
        self
    }

    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    pub fn with_data(mut self, data: impl Into<Value>) -> Self {
        self.data = Some(data.into());
        self
    }
}

impl fmt::Display for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.name, self.message)
    }
}

impl std::error::Error for ToolError {}

impl From<anyhow::Error> for ToolError {
    fn from(e: anyhow::Error) -> Self {
        ToolError::new(format!("{e:#}"))
    }
}

pub type ToolResult = Result<Value, ToolError>;

pub trait Handler: Send + Sync + 'static {
    fn call(&self, call: Call) -> BoxFuture<'static, ToolResult>;
}

impl<F, Fut> Handler for F
where
    F: Fn(Call) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ToolResult> + Send + 'static,
{
    fn call(&self, call: Call) -> BoxFuture<'static, ToolResult> {
        Box::pin((self)(call))
    }
}

/// Decides whether a call needs approval; `Some(reason)` means it does.
pub type ApprovalFn = Arc<dyn Fn(&Call) -> Option<String> + Send + Sync>;

/// When a call needs a human (or policy engine) decision before it runs.
#[derive(Clone, Default)]
pub enum Approval {
    /// Run without asking.
    #[default]
    Never,
    /// Suspend the run for approval before every call.
    Always,
    /// Suspend when the predicate returns a reason; run directly on `None`.
    When(ApprovalFn),
}

impl fmt::Debug for Approval {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Approval::Never => f.write_str("Never"),
            Approval::Always => f.write_str("Always"),
            Approval::When(_) => f.write_str("When(..)"),
        }
    }
}

#[derive(Clone)]
pub struct Tool {
    pub name: String,
    pub description: Option<String>,
    /// JSON Schema of the first argument, used to generate TypeScript
    /// declarations for the model.
    pub input_schema: Option<serde_json::Value>,
    /// JSON Schema of the result.
    pub output_schema: Option<serde_json::Value>,
    pub approval: Approval,
    pub(crate) handler: Arc<dyn Handler>,
}

impl Tool {
    pub fn new(name: impl Into<String>, handler: impl Handler) -> Self {
        Tool {
            name: name.into(),
            description: None,
            input_schema: None,
            output_schema: None,
            approval: Approval::Never,
            handler: Arc::new(handler),
        }
    }

    pub fn description(mut self, d: impl Into<String>) -> Self {
        self.description = Some(d.into());
        self
    }

    pub fn input_schema(mut self, schema: serde_json::Value) -> Self {
        self.input_schema = Some(schema);
        self
    }

    pub fn output_schema(mut self, schema: serde_json::Value) -> Self {
        self.output_schema = Some(schema);
        self
    }

    pub fn approval(mut self, approval: Approval) -> Self {
        self.approval = approval;
        self
    }

    /// Call the handler directly (outside any sandbox run).
    pub fn invoke(&self, call: Call) -> BoxFuture<'static, ToolResult> {
        self.handler.call(call)
    }

    pub fn needs_approval(&self, call: &Call) -> Option<String> {
        match &self.approval {
            Approval::Never => None,
            Approval::Always => Some(format!("{} requires approval", self.name)),
            Approval::When(f) => f(call),
        }
    }
}

/// Host-side state that belongs to the VM's timeline (e.g. a filesystem's
/// staged writes). It's sealed into continuation tokens and saved sessions
/// alongside the VM snapshot, and restored before a resume continues.
pub trait Extension: Send + Sync + 'static {
    /// Stable name; the key under which state is stored.
    fn name(&self) -> &str;
    fn save(&self) -> Vec<u8>;
    fn load(&self, data: &[u8]) -> Result<(), String>;
}

/// The set of tools a run may use.
#[derive(Clone, Default)]
pub struct Toolbox {
    tools: BTreeMap<String, Tool>,
    extensions: Vec<Arc<dyn Extension>>,
}

impl Toolbox {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a tool. Names are dotted paths of JS identifiers.
    pub fn add(&mut self, tool: Tool) -> &mut Self {
        assert!(valid_path(&tool.name), "invalid tool name {:?}: use dotted JS identifiers", tool.name);
        self.tools.insert(tool.name.clone(), tool);
        self
    }

    /// Shorthand for `add(Tool::new(name, handler))`.
    pub fn func(&mut self, name: impl Into<String>, handler: impl Handler) -> &mut Self {
        self.add(Tool::new(name, handler))
    }

    pub fn with(mut self, tool: Tool) -> Self {
        self.add(tool);
        self
    }

    /// Register state that must travel with suspended runs and sessions.
    pub fn extend(&mut self, ext: Arc<dyn Extension>) -> &mut Self {
        self.extensions.retain(|e| e.name() != ext.name());
        self.extensions.push(ext);
        self
    }

    pub fn extensions(&self) -> &[Arc<dyn Extension>] {
        &self.extensions
    }

    /// Snapshot all extension state (base64 per extension name).
    pub(crate) fn save_extensions(&self) -> BTreeMap<String, String> {
        use base64::Engine as _;
        self.extensions
            .iter()
            .map(|e| (e.name().to_owned(), base64::engine::general_purpose::STANDARD.encode(e.save())))
            .collect()
    }

    pub(crate) fn load_extensions(&self, saved: &BTreeMap<String, String>) -> Result<(), String> {
        use base64::Engine as _;
        for e in &self.extensions {
            if let Some(b64) = saved.get(e.name()) {
                let data = base64::engine::general_purpose::STANDARD
                    .decode(b64)
                    .map_err(|_| format!("corrupt state for {}", e.name()))?;
                e.load(&data).map_err(|m| format!("{}: {m}", e.name()))?;
            }
        }
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<&Tool> {
        self.tools.get(name)
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.tools.keys().map(String::as_str)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Tool> {
        self.tools.values()
    }

    pub fn len(&self) -> usize {
        self.tools.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// Hash of the tool names, bound into continuations so a token can't be
    /// resumed against a different capability set.
    pub(crate) fn scope_hash(&self) -> String {
        let mut h = blake3::Hasher::new();
        for name in self.tools.keys() {
            h.update(name.as_bytes());
            h.update(&[0]);
        }
        h.finalize().to_hex()[..16].to_owned()
    }
}

pub(crate) fn valid_path(path: &str) -> bool {
    !path.is_empty()
        && path.split('.').all(|seg| {
            let mut chars = seg.chars();
            matches!(chars.next(), Some(c) if c == '_' || c == '$' || c.is_ascii_alphabetic())
                && chars.all(|c| c == '_' || c == '$' || c.is_ascii_alphanumeric())
        })
}

#[cfg(test)]
mod tests {
    #[test]
    fn paths() {
        assert!(super::valid_path("tools.double"));
        assert!(super::valid_path("sh"));
        assert!(super::valid_path("a.b.c_d.$e"));
        assert!(!super::valid_path("a..b"));
        assert!(!super::valid_path("1a"));
        assert!(!super::valid_path("a-b"));
    }
}
