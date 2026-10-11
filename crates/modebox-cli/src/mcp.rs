//! `modebox mcp`: an MCP server that gives any agent harness "code mode".
//!
//! The model gets one `execute` tool. Its code runs in the modebox sandbox,
//! where every configured MCP server's tools (and an OS-sandboxed `sh`) are
//! typed async functions. Calls that need approval suspend the whole VM, and
//! the user is asked through the best channel the client supports:
//!
//! 1. MCP 2026-07-28 clients: an `input_required` result carrying an
//!    elicitation, with the sealed continuation as `requestState`. The server
//!    keeps no state, and an approval can come back after a restart.
//! 2. Clients that support elicitation: an inline elicitation prompt.
//! 3. Others: the calls are denied, since only the model could answer.
//!    With `mcp.approveTool` on, the run pauses instead and the `approve`
//!    tool resumes it; the harness's own permission prompt for that tool is
//!    then the user's decision.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use modebox_core::{Engine, Interrupt, Limits, Outcome, Resolution, RunOptions, RunResult, Session, Toolbox, Value, declarations};
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation, InitializeResult,
    InputRequest, InputRequiredResult, ListToolsResult, PaginatedRequestParams, ProtocolVersion, ServerCapabilities,
    Tool as McpTool, ToolAnnotations,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::{ErrorData as McpError, ServerHandler, ServiceExt};
use serde::Deserialize;
use serde_json::{Value as Json, json};

const MAX_RESULT_CHARS: usize = 40_000;
const MAX_DESCRIPTION_DECLS: usize = 16_000;

/// What sandboxed code sees when a call is denied for lack of a prompt.
const NO_PROMPT: &str = "this needs the user's approval, and this MCP client can't ask for it";

pub struct ModeboxServer {
    tools: Arc<Toolbox>,
    /// One persistent VM per connection: state carries across `execute` calls.
    session: tokio::sync::Mutex<Session>,
    decls: String,
    pending: Mutex<HashMap<String, PendingRun>>,
    /// Offer `approve` to clients that can't prompt (see `McpConfig`).
    approve_tool: bool,
    replays: Replays,
}

/// Results of resumes, by the token (or approval id) they used up. A client
/// that retries a request whose response it never got receives the same
/// result again instead of "already used"; a retry that arrives while the
/// first attempt is still running waits for it. The first decision wins.
#[derive(Default)]
struct Replays {
    entries: Mutex<HashMap<[u8; 32], Replay>>,
}

struct Replay {
    at: Instant,
    result: Arc<tokio::sync::OnceCell<CallToolResponse>>,
}

/// Retries come within seconds; keep results a while longer, and only a few.
const REPLAY_FOR: Duration = Duration::from_secs(600);
const REPLAY_MAX: usize = 256;

impl Replays {
    fn slot(&self, key: &str) -> Arc<tokio::sync::OnceCell<CallToolResponse>> {
        let k = *blake3::hash(key.as_bytes()).as_bytes();
        let now = Instant::now();
        let mut entries = self.entries.lock().unwrap();
        entries.retain(|_, r| now.duration_since(r.at) < REPLAY_FOR);
        if entries.len() >= REPLAY_MAX
            && !entries.contains_key(&k)
            && let Some(oldest) = entries.iter().min_by_key(|(_, r)| r.at).map(|(k, _)| *k)
        {
            entries.remove(&oldest);
        }
        entries.entry(k).or_insert_with(|| Replay { at: now, result: Default::default() }).result.clone()
    }
}

struct PendingRun {
    continuation: String,
    interrupts: Vec<Interrupt>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct ApprovalForm {
    /// Allow this action?
    approve: bool,
}
rmcp::elicit_safe!(ApprovalForm);

impl ModeboxServer {
    pub fn new(engine: Engine, tools: Toolbox, limits: Limits, approve_tool: bool) -> Self {
        let decls = declarations(&tools);
        let opts = RunOptions { limits: limits.clone(), filename: "execute.ts".into(), ..RunOptions::default() };
        let session = tokio::sync::Mutex::new(engine.session(tools.clone(), opts));
        ModeboxServer {
            session,
            tools: Arc::new(tools),
            decls,
            pending: Mutex::new(HashMap::new()),
            approve_tool,
            replays: Replays::default(),
        }
    }

    pub async fn serve_stdio(self) -> anyhow::Result<()> {
        let service = self.serve(rmcp::transport::stdio()).await?;
        service.waiting().await?;
        Ok(())
    }

    fn execute_description(&self) -> String {
        let api = if self.tools.is_empty() {
            "// (no APIs configured)".to_owned()
        } else if self.decls.len() <= MAX_DESCRIPTION_DECLS {
            self.decls.clone()
        } else {
            let mut ns: Vec<&str> = self.tools.names().map(|n| n.split('.').next().unwrap_or(n)).collect();
            ns.dedup();
            format!(
                "// {} functions in: {}. Call the `types` tool with a search term to see their signatures.",
                self.tools.len(),
                ns.join(", ")
            )
        };
        format!(
            "Run JavaScript/TypeScript in a secure sandbox. Prefer one `execute` that calls several APIs, \
filters and combines the results in code, and returns only what you need, over many separate tool calls.\n\n\
- Top-level `await` works. The result is the value of the last expression, or of a top-level `return`.\n\
- State persists between calls like a notebook: top-level `const`/`let`/`function` from earlier calls are still \
defined, so fetch data once and query it again later. Pass `fresh: true` to start over.\n\
- console.log output is returned as well.\n\
- No network, filesystem, require or import: only the functions below. They are async, so await them, \
and use Promise.all for independent calls.\n\
- Some calls may pause for the user's approval; that is expected.\n\n\
```ts\n{api}\n```"
        )
    }

    fn render(&self, r: &RunResult) -> CallToolResult {
        let mut text = String::new();
        let (is_error, structured) = match &r.outcome {
            Outcome::Completed(v) => {
                let j = v.to_json();
                let s = if let Json::String(s) = &j { s.clone() } else { serde_json::to_string_pretty(&j).unwrap_or_default() };
                text.push_str(&truncate(&s, MAX_RESULT_CHARS));
                (false, Some(json!({ "result": j })))
            }
            Outcome::Failed(e) => {
                text.push_str(&format!("{:?} error. {}: {}", e.kind, e.name, e.message));
                if let Some(st) = &e.stack {
                    text.push('\n');
                    text.push_str(st.trim_end());
                }
                (true, None)
            }
            Outcome::Suspended(_) => unreachable!("suspensions are handled before rendering"),
        };
        if !r.logs.is_empty() {
            text.push_str("\n\n--- console ---\n");
            let logs: Vec<String> = r
                .logs
                .iter()
                .map(|l| if l.level == "log" { l.message.clone() } else { format!("[{}] {}", l.level, l.message) })
                .collect();
            text.push_str(&truncate(&logs.join("\n"), MAX_RESULT_CHARS / 4));
        }
        let mut out = if is_error {
            CallToolResult::error(vec![ContentBlock::text(text)])
        } else {
            CallToolResult::success(vec![ContentBlock::text(text)])
        };
        out.structured_content = structured;
        out
    }

    async fn finish(
        &self,
        session: &mut Session,
        mut r: RunResult,
        ctx: &RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let mut denied: Vec<String> = Vec::new();
        loop {
            let s = match &r.outcome {
                Outcome::Suspended(s) => s.clone(),
                _ => {
                    let mut result = self.render(&r);
                    if !denied.is_empty() {
                        result.content.push(ContentBlock::text(no_prompt_note(&denied)));
                    }
                    return Ok(CallToolResponse::Complete(result));
                }
            };
            if supports_mrtr(ctx) {
                return Ok(CallToolResponse::InputRequired(mrtr_request(&s.interrupts, &s.continuation)));
            }
            if supports_elicitation(ctx) {
                let mut decisions = Vec::new();
                for i in &s.interrupts {
                    let res = ctx.peer.elicit::<ApprovalForm>(describe(i)).await;
                    decisions.push((i.id, match res {
                        Ok(Some(ApprovalForm { approve: true })) => Resolution::Approve,
                        _ => Resolution::Deny("the user declined".into()),
                    }));
                }
                r = session.resume(&s.continuation, decisions).await;
                continue;
            }
            if !self.approve_tool {
                // Nobody but the model could answer, so the answer is no.
                denied.extend(s.interrupts.iter().map(describe));
                let decisions = s.interrupts.iter().map(|i| (i.id, Resolution::Deny(NO_PROMPT.into()))).collect::<Vec<_>>();
                r = session.resume(&s.continuation, decisions).await;
                continue;
            }
            let id = format!("run-{}", &short_hash(&s.continuation)[..10]);
            let listing: Vec<String> = s.interrupts.iter().map(|i| format!("- call {}: {}", i.id, describe(i))).collect();
            let per_call = if s.interrupts.len() > 1 {
                let example: Vec<String> = s.interrupts.iter().map(|i| format!("\"{}\": true", i.id)).collect();
                format!(
                    " To decide on each call, pass {{\"id\": \"{id}\", \"decisions\": {{{}}}}} with false for the ones the user declined.",
                    example.join(", ")
                )
            } else {
                String::new()
            };
            let msg = format!(
                "Paused: this code needs the user's approval before it continues.\n{}\n\n\
Ask the user. To continue, call the `approve` tool with {{\"id\": \"{id}\", \"approve\": true}} \
(or false to deny).{per_call} The sandbox state is saved; nothing before this point will run again.",
                listing.join("\n")
            );
            self.pending.lock().unwrap().insert(id, PendingRun { continuation: s.continuation, interrupts: s.interrupts });
            return Ok(CallToolResponse::Complete(CallToolResult::success(vec![ContentBlock::text(msg)])));
        }
    }
}

fn no_prompt_note(denied: &[String]) -> String {
    let list: Vec<String> = denied.iter().map(|d| format!("- {d}")).collect();
    format!(
        "modebox denied {} call(s) that need the user's approval, because this MCP client can't ask the user \
(it doesn't support elicitation):\n{}\n\nIf the harness asks the user before each tool call, set \
\"mcp\": {{ \"approveTool\": true }} in modebox.json: runs will pause and an `approve` tool will resume them.",
        denied.len(),
        list.join("\n")
    )
}

fn short_hash(s: &str) -> String {
    // Short stable id for a continuation (not security sensitive).
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_owned();
    }
    let mut cut = max;
    while !s.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}\n… ({} more characters truncated; return less data)", &s[..cut], s.len() - cut)
}

fn describe(i: &Interrupt) -> String {
    let args = Value::Array(i.args.clone()).to_json();
    let reason = i.payload.get("reason").and_then(Value::as_str).unwrap_or(&i.kind);
    format!("{} wants to call {}({}) [{}]", "Sandboxed code", i.tool, truncate(args.to_string().trim_matches(['[', ']']), 400), reason)
}

fn supports_mrtr(ctx: &RequestContext<RoleServer>) -> bool {
    ctx.protocol_version().is_some_and(|v| v.to_string().as_str() >= ProtocolVersion::V_2026_07_28.to_string().as_str())
}

fn supports_elicitation(ctx: &RequestContext<RoleServer>) -> bool {
    ctx.peer.peer_info().is_some_and(|p| p.capabilities.elicitation.is_some())
}

fn mrtr_request(interrupts: &[Interrupt], continuation: &str) -> InputRequiredResult {
    let mut requests = BTreeMap::new();
    for i in interrupts {
        let req: InputRequest = serde_json::from_value(json!({
            "method": "elicitation/create",
            "params": {
                "mode": "form",
                "message": describe(i),
                "requestedSchema": {
                    "type": "object",
                    "properties": { "approve": { "type": "boolean", "title": "Approve", "description": "Allow this action?" } },
                    "required": ["approve"]
                }
            }
        }))
        .expect("valid elicitation request");
        requests.insert(format!("approve-{}", i.id), req);
    }
    InputRequiredResult::new(Some(requests), Some(continuation.to_owned()))
}

fn decisions_from_responses(responses: &BTreeMap<String, Json>) -> Vec<(u32, Resolution)> {
    responses
        .iter()
        .filter_map(|(k, v)| {
            let id: u32 = k.strip_prefix("approve-")?.parse().ok()?;
            let ok = v["action"] == "accept" && v["content"]["approve"] == true;
            Some((id, if ok { Resolution::Approve } else { Resolution::Deny("the user declined".into()) }))
        })
        .collect()
}

fn schema(v: Json) -> Arc<serde_json::Map<String, Json>> {
    match v {
        Json::Object(m) => Arc::new(m),
        _ => Arc::new(Default::default()),
    }
}

impl ServerHandler for ModeboxServer {
    fn get_info(&self) -> InitializeResult {
        InitializeResult::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("modebox", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "modebox runs your code in a WebAssembly sandbox with typed access to configured tools. \
Use `execute` to batch tool calls and data processing into one step.",
            )
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        // Hints for harnesses that decide which tools to allow without asking:
        // `execute` can reach every configured tool, and `approve` releases
        // whatever a run was paused on, so neither is safe to auto-allow.
        let mut tools = vec![
            McpTool::new(
                "execute",
                self.execute_description(),
                schema(json!({
                    "type": "object",
                    "properties": {
                        "code": { "type": "string", "description": "JavaScript or TypeScript to run" },
                        "fresh": { "type": "boolean", "description": "Discard state from earlier calls first" }
                    },
                    "required": ["code"]
                })),
            )
            .annotate(
                ToolAnnotations::with_title("Run code in the modebox sandbox")
                    .read_only(false)
                    .destructive(true)
                    .idempotent(false)
                    .open_world(true),
            ),
            McpTool::new(
                "types",
                "Show TypeScript signatures of the sandbox APIs, optionally filtered by a search term.",
                schema(json!({ "type": "object", "properties": { "query": { "type": "string" } } })),
            )
            .annotate(ToolAnnotations::with_title("Show sandbox API types").read_only(true).open_world(false)),
        ];
        if self.approve_tool {
            tools.push(
                McpTool::new(
                    "approve",
                    "Continue a paused `execute` run with the user's decision. Ask the user first and pass exactly \
what they said; never decide on their behalf.",
                    schema(json!({
                        "type": "object",
                        "properties": {
                            "id": { "type": "string", "description": "The paused run's id" },
                            "approve": { "type": "boolean", "description": "The user's decision for every pending call" },
                            "decisions": {
                                "type": "object",
                                "description": "The user's decision per call id, e.g. {\"3\": true, \"4\": false}; calls left out are denied",
                                "additionalProperties": { "type": "boolean" }
                            }
                        },
                        "required": ["id"]
                    })),
                )
                .annotate(
                    ToolAnnotations::with_title("Approve a paused sandbox run")
                        .read_only(false)
                        .destructive(true)
                        .idempotent(false)
                        .open_world(true),
                ),
            );
        }
        Ok(ListToolsResult::with_all_items(tools))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let args = Json::Object(request.arguments.clone().unwrap_or_default());
        match request.name.as_ref() {
            "execute" => {
                if let Some(state) = &request.request_state {
                    let slot = self.replays.slot(&format!("state:{state}"));
                    let responses = request.input_responses.clone().unwrap_or_default();
                    let result = slot
                        .get_or_try_init(|| async {
                            let mut session = self.session.lock().await;
                            let r = session.resume(state, decisions_from_responses(&responses)).await;
                            self.finish(&mut session, r, &context).await
                        })
                        .await?;
                    return Ok(result.clone());
                }
                let mut session = self.session.lock().await;
                let Some(code) = args["code"].as_str() else {
                    return Err(McpError::invalid_params("execute: `code` must be a string", None));
                };
                if args["fresh"] == true {
                    session.reset();
                }
                let r = session.run(code).await;
                self.finish(&mut session, r, &context).await
            }
            "types" => {
                let q = args["query"].as_str().unwrap_or("").to_lowercase();
                let text = if q.is_empty() {
                    self.decls.clone()
                } else {
                    let mut sub = Toolbox::new();
                    for t in self.tools.iter() {
                        let hay = format!("{} {}", t.name, t.description.clone().unwrap_or_default()).to_lowercase();
                        if hay.contains(&q) {
                            sub.add(t.clone());
                        }
                    }
                    declarations(&sub)
                };
                Ok(CallToolResponse::Complete(CallToolResult::success(vec![ContentBlock::text(if text.is_empty() {
                    "no matching functions".to_owned()
                } else {
                    text
                })])))
            }
            "approve" if self.approve_tool => {
                let id = args["id"].as_str().unwrap_or("");
                let all = args["approve"].as_bool().unwrap_or(false);
                let per_call = args["decisions"].as_object().cloned();
                let slot = self.replays.slot(&format!("approve:{id}"));
                let result = slot
                    .get_or_try_init(|| async {
                        let Some(p) = self.pending.lock().unwrap().remove(id) else {
                            return Err(McpError::invalid_params(format!("no paused run with id {id}"), None));
                        };
                        let decisions = p
                            .interrupts
                            .iter()
                            .map(|i| {
                                let ok = match &per_call {
                                    Some(d) => d.get(&i.id.to_string()).and_then(Json::as_bool) == Some(true),
                                    None => all,
                                };
                                (i.id, if ok { Resolution::Approve } else { Resolution::Deny("the user declined".into()) })
                            })
                            .collect::<Vec<_>>();
                        let mut session = self.session.lock().await;
                        let r = session.resume(&p.continuation, decisions).await;
                        self.finish(&mut session, r, &context).await
                    })
                    .await?;
                Ok(result.clone())
            }
            other => Err(McpError::invalid_params(format!("unknown tool {other}"), None)),
        }
    }
}
