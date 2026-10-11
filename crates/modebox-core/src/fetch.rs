//! `fetch()` for sandboxed code: HTTP requests the host makes on its behalf.
//!
//! The guest's `fetch` calls the `$fetch` tool. Hosts on the allow list are
//! fetched directly; others need approval (or are refused). Every address is
//! checked when it's resolved, so DNS can't point an allowed name at the
//! local network: only exact allow-list entries may reach private addresses.
//! Redirects are followed only to hosts the request could reach anyway.
//!
//! Embedders can also implement `$fetch` themselves. It receives
//! `{ url, method, headers: [[name, value]...], body?: Uint8Array, redirect }`
//! and returns `{ url, status, statusText, headers, body: Uint8Array,
//! redirected }`.

#![allow(clippy::result_large_err)] // ToolError carries messages; boxing it buys nothing here.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use reqwest::dns::{Addrs, Name, Resolve, Resolving};

use crate::net::{HostAllow, is_public};
use crate::tools::{Approval, Call, Tool, ToolError};
use crate::value::Value;

/// The tool name the guest's `fetch` calls.
pub const FETCH_TOOL: &str = "$fetch";

#[derive(Clone, Debug)]
pub struct FetchPolicy {
    /// Hosts fetched without asking (see [`HostAllow`]).
    pub allow: Vec<String>,
    /// Other hosts: ask for approval (`true`) or refuse them.
    pub ask_for_others: bool,
    /// Largest response body; bigger ones fail the request.
    pub max_response_bytes: usize,
    pub timeout: Duration,
    pub max_redirects: usize,
}

impl Default for FetchPolicy {
    fn default() -> Self {
        FetchPolicy {
            allow: Vec::new(),
            ask_for_others: true,
            max_response_bytes: 8 << 20,
            timeout: Duration::from_secs(30),
            max_redirects: 10,
        }
    }
}

/// The `$fetch` tool. It's a sink (see [`Tool::sink`]): once a run has read
/// private data, fetching needs approval.
pub fn fetch_tool(policy: FetchPolicy) -> Tool {
    let allow = Arc::new(HostAllow::new(&policy.allow));
    let check = allow.clone();
    let ask = policy.ask_for_others;
    let policy = Arc::new(policy);
    Tool::new(FETCH_TOOL, move |call: Call| {
        let (policy, allow) = (policy.clone(), allow.clone());
        async move { fetch(&policy, &allow, &call).await }
    })
    .description("Make an HTTP request for the sandbox's fetch()")
    .approval(Approval::When(Arc::new(move |call: &Call| {
        let host = request_url(call).ok()?.host_str()?.to_owned();
        (ask && check.check(&host).is_none()).then(|| format!("fetch from {host}, which isn't on the allow list"))
    })))
    .sink()
}

fn type_error(msg: impl Into<String>) -> ToolError {
    ToolError::new(msg).with_name("TypeError")
}

fn request_url(call: &Call) -> Result<url::Url, ToolError> {
    let raw = call.input().get("url").and_then(Value::as_str).ok_or_else(|| type_error("fetch: no url"))?;
    let url = url::Url::parse(raw).map_err(|e| type_error(format!("fetch: invalid URL {raw}: {e}")))?;
    match url.scheme() {
        "http" | "https" => Ok(url),
        other => Err(type_error(format!("fetch: {other}: URLs aren't supported"))),
    }
}

/// Which hosts one request may reach: the allow list, plus the host it was
/// approved for (public addresses only, since the approval named a host, not
/// an address).
#[derive(Clone)]
struct Reach {
    allow: Arc<HostAllow>,
    approved: Option<String>,
}

impl Reach {
    /// `None` if the host is out of reach; otherwise whether private addresses are allowed.
    fn check(&self, host: &str) -> Option<bool> {
        let host = host.trim_start_matches('[').trim_end_matches(']').trim_end_matches('.').to_ascii_lowercase();
        self.allow.check(&host).or_else(|| (self.approved.as_deref() == Some(host.as_str())).then_some(false))
    }

    /// For URLs whose host is an IP address, which skip DNS.
    fn check_literal(&self, url: &url::Url) -> Result<(), String> {
        let host = url.host_str().unwrap_or_default();
        let private_ok = self.check(host).ok_or_else(|| format!("{host} isn't on the allow list"))?;
        if let Ok(ip) = host.trim_start_matches('[').trim_end_matches(']').parse::<IpAddr>()
            && !private_ok
            && !is_public(ip)
        {
            return Err(format!("{host} is a non-public address; list it exactly to allow that"));
        }
        Ok(())
    }
}

/// Resolves only hosts in reach, and drops non-public addresses unless the
/// host is listed exactly. Checked at connect time, for every redirect too.
impl Resolve for Reach {
    fn resolve(&self, name: Name) -> Resolving {
        let reach = self.clone();
        let host = name.as_str().to_owned();
        Box::pin(async move {
            let private_ok = reach.check(&host).ok_or_else(|| format!("{host} isn't on the allow list"))?;
            let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 0))
                .await?
                .filter(|a| private_ok || is_public(a.ip()))
                .collect();
            if addrs.is_empty() {
                return Err(format!("{host} has no public address; list it exactly to allow private ones").into());
            }
            Ok(Box::new(addrs.into_iter()) as Addrs)
        })
    }
}

/// Headers the client manages; values sandboxed code sets are dropped.
const MANAGED: &[&str] = &["host", "content-length", "connection", "transfer-encoding", "keep-alive", "upgrade", "te", "trailer"];

async fn fetch(policy: &FetchPolicy, allow: &Arc<HostAllow>, call: &Call) -> Result<Value, ToolError> {
    let input = call.input();
    let url = request_url(call)?;
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    let reach = Reach {
        allow: allow.clone(),
        // Not on the list, yet running: the call was approved (see `fetch_tool`).
        approved: (policy.ask_for_others && allow.check(&host).is_none()).then(|| host.clone()),
    };
    reach.check_literal(&url).map_err(|why| type_error(format!("fetch: {why}")))?;

    let redirect = match input.get("redirect").and_then(Value::as_str).unwrap_or("follow") {
        "manual" => reqwest::redirect::Policy::none(),
        mode => {
            let (reach, max, error) = (reach.clone(), policy.max_redirects, mode == "error");
            reqwest::redirect::Policy::custom(move |attempt| {
                if error {
                    return attempt.error("fetch: the response is a redirect and redirect is \"error\"");
                }
                if attempt.previous().len() >= max {
                    return attempt.error("fetch: too many redirects");
                }
                match reach.check_literal(attempt.url()) {
                    Ok(()) => attempt.follow(),
                    Err(why) => attempt.error(format!("fetch: redirected to a host out of reach: {why}")),
                }
            })
        }
    };
    let client = reqwest::Client::builder()
        .dns_resolver(Arc::new(reach))
        .redirect(redirect)
        .timeout(policy.timeout)
        .no_proxy()
        .build()
        .map_err(|e| type_error(format!("fetch: {e}")))?;

    let method = input.get("method").and_then(Value::as_str).unwrap_or("GET");
    let method = reqwest::Method::from_bytes(method.to_ascii_uppercase().as_bytes()).map_err(|_| type_error(format!("fetch: invalid method {method}")))?;
    let mut req = client.request(method, url);
    if let Some(Value::Array(headers)) = input.get("headers") {
        for pair in headers {
            let Value::Array(kv) = pair else { continue };
            let (Some(name), Some(value)) = (kv.first().and_then(Value::as_str), kv.get(1).and_then(Value::as_str)) else { continue };
            if !MANAGED.contains(&name.to_ascii_lowercase().as_str()) {
                req = req.header(name, value);
            }
        }
    }
    match input.get("body") {
        Some(Value::Bytes { data, .. }) => req = req.body(data.clone()),
        Some(Value::String(s)) => req = req.body(s.clone()),
        _ => {}
    }

    let mut resp = req.send().await.map_err(|e| type_error(format!("fetch failed: {}", describe(&e))))?;
    let status = resp.status();
    let final_url = resp.url().to_string();
    let headers: Vec<Value> = resp
        .headers()
        .iter()
        .map(|(k, v)| Value::Array(vec![Value::from(k.as_str()), Value::from(String::from_utf8_lossy(v.as_bytes()).into_owned())]))
        .collect();
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|e| type_error(format!("fetch failed: {}", describe(&e))))? {
        if body.len() + chunk.len() > policy.max_response_bytes {
            return Err(type_error(format!("fetch: the response is larger than {} bytes", policy.max_response_bytes)));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(Value::object([
        ("url", Value::from(final_url.clone())),
        ("status", Value::from(status.as_u16())),
        ("statusText", Value::from(status.canonical_reason().unwrap_or(""))),
        ("headers", Value::Array(headers)),
        ("body", Value::Bytes { kind: "Uint8Array".into(), data: body }),
        ("redirected", Value::from(final_url != request_url(call)?.as_str())),
    ]))
}

/// An error with its causes, which carry the useful part (a refused host).
fn describe(e: &reqwest::Error) -> String {
    let mut s = e.to_string();
    let mut source = std::error::Error::source(e);
    while let Some(cause) = source {
        s.push_str(": ");
        s.push_str(&cause.to_string());
        source = cause.source();
    }
    s
}
