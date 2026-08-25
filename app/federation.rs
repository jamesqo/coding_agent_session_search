use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::io::{Read, Write};
use std::process::{Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::AppError;
use crate::cli::{SearchResponse, ViewResponse};
use crate::storage::SearchHit;

const PROTOCOL_VERSION: u8 = 2;
const MAX_NODES: usize = 16;
const REMOTE_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_STDOUT_BYTES: usize = 16 * 1024 * 1024;
const MAX_STDERR_BYTES: usize = 64 * 1024;

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct SearchRequest {
    pub(crate) protocol: u8,
    pub(crate) query: String,
    pub(crate) limit: usize,
    pub(crate) provider: Option<String>,
    pub(crate) days: Option<u32>,
}

impl SearchRequest {
    pub(crate) fn new(
        query: String,
        limit: usize,
        provider: Option<String>,
        days: Option<u32>,
    ) -> Self {
        Self {
            protocol: PROTOCOL_VERSION,
            query,
            limit,
            provider,
            days,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct ViewRequest {
    pub(crate) protocol: u8,
    pub(crate) id: String,
    pub(crate) context: u32,
}

impl ViewRequest {
    pub(crate) fn new(id: String, context: u32) -> Self {
        Self {
            protocol: PROTOCOL_VERSION,
            id,
            context,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct SearchEnvelope {
    protocol: u8,
    kind: String,
    pub(crate) response: SearchResponse,
}

impl SearchEnvelope {
    pub(crate) fn new(response: SearchResponse) -> Self {
        Self {
            protocol: PROTOCOL_VERSION,
            kind: "search".to_owned(),
            response,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct ViewEnvelope {
    protocol: u8,
    kind: String,
    pub(crate) response: ViewResponse,
}

impl ViewEnvelope {
    pub(crate) fn new(response: ViewResponse) -> Self {
        Self {
            protocol: PROTOCOL_VERSION,
            kind: "view".to_owned(),
            response,
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct NodeOutcome {
    pub(crate) node: String,
    pub(crate) status: String,
    pub(crate) elapsed_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) realized_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) error_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<String>,
}

pub(crate) struct RemoteResult<T> {
    pub(crate) response: Option<T>,
    pub(crate) outcome: NodeOutcome,
}

pub(crate) fn thread_failure<T>(node: String) -> RemoteResult<T> {
    remote_error(
        node,
        0,
        "worker",
        "remote search worker panicked".to_owned(),
    )
}

struct ProcessOutput {
    status: ExitStatus,
    stdout: BoundedOutput,
    stderr: BoundedOutput,
    timed_out: bool,
    elapsed_ms: u64,
}

struct BoundedOutput {
    bytes: Vec<u8>,
    overflowed: bool,
}

struct MergedHit {
    hit: SearchHit,
    score: f64,
}

#[derive(Deserialize)]
struct RemoteErrorEnvelope {
    error: RemoteErrorBody,
}

#[derive(Deserialize)]
struct RemoteErrorBody {
    kind: String,
    message: String,
}

pub(crate) fn select_nodes(explicit: &[String]) -> Result<Vec<String>, AppError> {
    let candidates = if explicit.is_empty() {
        std::env::var("CASS_SEARCH_NODES")
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|node| !node.is_empty())
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>()
    } else {
        explicit.to_vec()
    };
    let mut seen = BTreeSet::new();
    let mut nodes = Vec::new();
    for node in candidates {
        validate_node(&node)?;
        if seen.insert(node.clone()) {
            nodes.push(node);
        }
    }
    if nodes.len() > MAX_NODES {
        return Err(AppError::usage(
            "federated search supports at most 16 remote nodes",
        ));
    }
    Ok(nodes)
}

pub(crate) fn validate_node(node: &str) -> Result<(), AppError> {
    let mut characters = node.chars();
    let valid_start = characters
        .next()
        .is_some_and(|character| character.is_ascii_alphanumeric());
    let valid_rest = characters
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-'));
    if !valid_start || !valid_rest || node == "local" {
        return Err(AppError::usage(format!("invalid SSH node: {node}")));
    }
    Ok(())
}

pub(crate) fn validate_protocol(protocol: u8) -> Result<(), AppError> {
    if protocol == PROTOCOL_VERSION {
        Ok(())
    } else {
        Err(AppError::usage(format!(
            "unsupported federation protocol version: {protocol}"
        )))
    }
}

pub(crate) fn merge_search(
    local: SearchResponse,
    remotes: Vec<RemoteResult<SearchResponse>>,
    limit: usize,
) -> SearchResponse {
    let mut merged = BTreeMap::<(String, String, String), MergedHit>::new();
    merge_ranked_results(&mut merged, "local", local.results);
    let mut outcomes = Vec::with_capacity(remotes.len());
    for mut remote in remotes {
        if let Some(response) = remote.response.take() {
            remote.outcome.realized_mode = Some(response.realized_mode);
            merge_ranked_results(&mut merged, &remote.outcome.node, response.results);
        }
        outcomes.push(remote.outcome);
    }
    let mut results = merged.into_values().collect::<Vec<_>>();
    results.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| left.hit.provider.cmp(&right.hit.provider))
            .then_with(|| left.hit.conversation_id.cmp(&right.hit.conversation_id))
            .then_with(|| left.hit.id.cmp(&right.hit.id))
    });
    results.truncate(limit);
    let results = results
        .into_iter()
        .map(|mut merged| {
            merged.hit.federated_score = Some(merged.score);
            merged.hit
        })
        .collect();
    SearchResponse {
        query: local.query,
        realized_mode: "federated".to_owned(),
        results,
        nodes: Some(outcomes),
    }
}

fn merge_ranked_results(
    merged: &mut BTreeMap<(String, String, String), MergedHit>,
    origin: &str,
    results: Vec<SearchHit>,
) {
    for (rank, mut hit) in results.into_iter().enumerate() {
        let denominator = rank
            .checked_add(1)
            .and_then(|rank| u32::try_from(rank).ok())
            .map_or(f64::from(u32::MAX), f64::from);
        let score = 1.0 / denominator;
        let key = (
            hit.provider.clone(),
            hit.conversation_id.clone(),
            hit.id.clone(),
        );
        if let Some(existing) = merged.get_mut(&key) {
            if !existing
                .hit
                .origins
                .iter()
                .any(|existing| existing == origin)
            {
                existing.hit.origins.push(origin.to_owned());
            }
            existing.score = existing.score.max(score);
        } else {
            hit.origins.push(origin.to_owned());
            merged.insert(key, MergedHit { hit, score });
        }
    }
}

pub(crate) fn remote_search(node: String, request: &SearchRequest) -> RemoteResult<SearchResponse> {
    remote_search_with(OsStr::new("ssh"), node, request, REMOTE_TIMEOUT)
}

fn remote_search_with(
    ssh: &OsStr,
    node: String,
    request: &SearchRequest,
    timeout: Duration,
) -> RemoteResult<SearchResponse> {
    let mut result = remote_call(
        ssh,
        node,
        "search",
        request,
        timeout,
        "search",
        |envelope: SearchEnvelope| envelope.response,
    );
    if result
        .response
        .as_ref()
        .is_some_and(|response| response.realized_mode != "hybrid")
    {
        return remote_error(
            result.outcome.node,
            result.outcome.elapsed_ms,
            "incompatible-response",
            "remote search did not report hybrid realization".to_owned(),
        );
    }
    if result.response.is_some() {
        result.outcome.realized_mode = Some("hybrid".to_owned());
    }
    result
}

pub(crate) fn remote_view(node: String, request: &ViewRequest) -> RemoteResult<ViewResponse> {
    remote_call(
        OsStr::new("ssh"),
        node,
        "view",
        request,
        REMOTE_TIMEOUT,
        "view",
        |envelope: ViewEnvelope| envelope.response,
    )
}

fn remote_call<Request, Envelope, Response>(
    ssh: &OsStr,
    node: String,
    operation: &'static str,
    request: &Request,
    timeout: Duration,
    expected_kind: &'static str,
    extract: fn(Envelope) -> Response,
) -> RemoteResult<Response>
where
    Request: Serialize,
    Envelope: DeserializeOwned + EnvelopeMetadata,
{
    let started = Instant::now();
    let payload = match serde_json::to_vec(request) {
        Ok(mut payload) => {
            payload.push(b'\n');
            payload
        }
        Err(error) => {
            return remote_error(
                node,
                elapsed_millis(started),
                "request",
                format!("failed to encode federation request: {error}"),
            );
        }
    };
    let process = match run_ssh(ssh, &node, operation, &payload, timeout) {
        Ok(process) => process,
        Err(error) => {
            return remote_error(node, elapsed_millis(started), "spawn", error);
        }
    };
    if process.timed_out {
        return remote_error(
            node,
            process.elapsed_ms,
            "timeout",
            "remote request exceeded five-second deadline".to_owned(),
        );
    }
    if process.stdout.overflowed {
        return remote_error(
            node,
            process.elapsed_ms,
            "output-too-large",
            "remote stdout exceeded 16 MiB".to_owned(),
        );
    }
    if !process.status.success() {
        let diagnostic = bounded_diagnostic(&process.stderr.bytes);
        if let Some(remote) = parse_remote_error(&process.stderr.bytes) {
            return remote_error(
                node,
                process.elapsed_ms,
                remote.error.kind,
                remote.error.message,
            );
        }
        return remote_error(
            node,
            process.elapsed_ms,
            "remote-exit",
            if diagnostic.is_empty() {
                format!("remote SSH command exited with {}", process.status)
            } else {
                diagnostic
            },
        );
    }
    let envelope: Envelope = match serde_json::from_slice(&process.stdout.bytes) {
        Ok(envelope) => envelope,
        Err(error) => {
            return remote_error(
                node,
                process.elapsed_ms,
                "malformed-response",
                format!("remote returned invalid JSON: {error}"),
            );
        }
    };
    if envelope.protocol() != PROTOCOL_VERSION || envelope.kind() != expected_kind {
        return remote_error(
            node,
            process.elapsed_ms,
            "incompatible-response",
            "remote returned an incompatible federation envelope".to_owned(),
        );
    }
    let response = extract(envelope);
    RemoteResult {
        outcome: NodeOutcome {
            node,
            status: "ok".to_owned(),
            elapsed_ms: process.elapsed_ms,
            realized_mode: None,
            error_kind: None,
            error: None,
        },
        response: Some(response),
    }
}

trait EnvelopeMetadata {
    fn protocol(&self) -> u8;
    fn kind(&self) -> &str;
}

impl EnvelopeMetadata for SearchEnvelope {
    fn protocol(&self) -> u8 {
        self.protocol
    }

    fn kind(&self) -> &str {
        &self.kind
    }
}

impl EnvelopeMetadata for ViewEnvelope {
    fn protocol(&self) -> u8 {
        self.protocol
    }

    fn kind(&self) -> &str {
        &self.kind
    }
}

fn remote_error<T>(
    node: String,
    elapsed_ms: u64,
    error_kind: impl Into<String>,
    error: String,
) -> RemoteResult<T> {
    RemoteResult {
        response: None,
        outcome: NodeOutcome {
            node,
            status: "error".to_owned(),
            elapsed_ms,
            realized_mode: None,
            error_kind: Some(error_kind.into()),
            error: Some(error),
        },
    }
}

fn run_ssh(
    ssh: &OsStr,
    node: &str,
    operation: &str,
    payload: &[u8],
    timeout: Duration,
) -> Result<ProcessOutput, String> {
    let started = Instant::now();
    let mut child = Command::new(ssh)
        .args([
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=5",
            "-o",
            "ServerAliveInterval=2",
            "-o",
            "ServerAliveCountMax=2",
            "--",
            node,
            "~/.local/bin/cass",
            operation,
            "--federation-request",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("failed to start ssh: {error}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "failed to capture remote stdout".to_owned())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "failed to capture remote stderr".to_owned())?;
    let stdout_reader = thread::spawn(move || read_bounded(stdout, MAX_STDOUT_BYTES));
    let stderr_reader = thread::spawn(move || read_bounded(stderr, MAX_STDERR_BYTES));
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(payload);
    }
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < timeout => thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                timed_out = true;
                let _ = child.kill();
                break child
                    .wait()
                    .map_err(|error| format!("failed to reap timed-out ssh: {error}"))?;
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("failed while waiting for ssh: {error}"));
            }
        }
    };
    let stdout = stdout_reader
        .join()
        .map_err(|_| "remote stdout reader panicked".to_owned())?
        .map_err(|error| format!("failed to read remote stdout: {error}"))?;
    let stderr = stderr_reader
        .join()
        .map_err(|_| "remote stderr reader panicked".to_owned())?
        .map_err(|error| format!("failed to read remote stderr: {error}"))?;
    Ok(ProcessOutput {
        status,
        stdout,
        stderr,
        timed_out,
        elapsed_ms: elapsed_millis(started),
    })
}

fn read_bounded(mut reader: impl Read, limit: usize) -> std::io::Result<BoundedOutput> {
    let mut output = Vec::new();
    let mut overflowed = false;
    let mut chunk = [0_u8; 16 * 1024];
    loop {
        let read = reader.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        let remaining = limit.saturating_sub(output.len());
        let retained = read.min(remaining);
        output.extend_from_slice(&chunk[..retained]);
        overflowed |= retained != read;
    }
    Ok(BoundedOutput {
        bytes: output,
        overflowed,
    })
}

fn elapsed_millis(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn bounded_diagnostic(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).trim().to_owned()
}

fn parse_remote_error(bytes: &[u8]) -> Option<RemoteErrorEnvelope> {
    bytes
        .split(|byte| *byte == b'\n')
        .rev()
        .find(|line| !line.iter().all(u8::is_ascii_whitespace))
        .and_then(|line| serde_json::from_slice(line).ok())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    use super::*;
    use veritas_test_macros as veritas;

    #[test]
    fn node_alias_validation_covers_boundaries() {
        for valid in ["xenia", "dev-macbook", "host.example", "node_1", "9node"] {
            assert!(validate_node(valid).is_ok(), "expected valid node: {valid}");
        }
        for invalid in ["", "local", "-option", "bad host", "bad;host", "nødé"] {
            assert!(
                validate_node(invalid).is_err(),
                "expected invalid node: {invalid}"
            );
        }
    }

    #[test]
    fn explicit_node_selection_is_stable_and_bounded() {
        let nodes = select_nodes(&[
            "dev-macbook".to_owned(),
            "xenia".to_owned(),
            "dev-macbook".to_owned(),
        ])
        .expect("valid nodes");
        assert_eq!(nodes, ["dev-macbook", "xenia"]);
        let excessive = (0..17)
            .map(|index| format!("node-{index}"))
            .collect::<Vec<_>>();
        assert!(select_nodes(&excessive).is_err());
    }

    #[test]
    fn process_runner_classifies_timeout_and_malformed_output() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let timeout = fake_executable(directory.path(), "timeout", "while :; do :; done\n");
        let request = SearchRequest::new("query".to_owned(), 1, None, None);
        let timed_out = remote_call(
            timeout.as_os_str(),
            "node".to_owned(),
            "search",
            &request,
            Duration::from_millis(30),
            "search",
            |envelope: SearchEnvelope| envelope.response,
        );
        assert_eq!(timed_out.outcome.error_kind.as_deref(), Some("timeout"));

        let malformed = fake_executable(directory.path(), "malformed", "printf 'not-json\\n'\n");
        let malformed = remote_call(
            malformed.as_os_str(),
            "node".to_owned(),
            "search",
            &request,
            Duration::from_secs(1),
            "search",
            |envelope: SearchEnvelope| envelope.response,
        );
        assert_eq!(
            malformed.outcome.error_kind.as_deref(),
            Some("malformed-response")
        );

        let nonzero = fake_executable(directory.path(), "nonzero", "exit 7\n");
        let nonzero = remote_call(
            nonzero.as_os_str(),
            "node".to_owned(),
            "search",
            &request,
            Duration::from_secs(1),
            "search",
            |envelope: SearchEnvelope| envelope.response,
        );
        assert_eq!(nonzero.outcome.error_kind.as_deref(), Some("remote-exit"));

        let oversized = fake_executable(
            directory.path(),
            "oversized",
            "dd if=/dev/zero bs=1048576 count=17 2>/dev/null\n",
        );
        let oversized = remote_call(
            oversized.as_os_str(),
            "node".to_owned(),
            "search",
            &request,
            Duration::from_secs(2),
            "search",
            |envelope: SearchEnvelope| envelope.response,
        );
        assert_eq!(
            oversized.outcome.error_kind.as_deref(),
            Some("output-too-large")
        );
    }

    #[veritas::claims(
        "federated-search/semantic-unready-node-is-partial-failure",
        "federated-search/successful-nodes-are-hybrid"
    )]
    #[test]
    fn remote_search_accepts_only_v2_hybrid_successes() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let request = SearchRequest::new("query".to_owned(), 1, None, None);
        for (name, body, expected) in [
            (
                "v1",
                "cat >/dev/null; printf '%s\\n' '{\"protocol\":1,\"kind\":\"search\",\"response\":{\"query\":\"query\",\"realized_mode\":\"hybrid\",\"results\":[]}}'\n",
                Some("incompatible-response"),
            ),
            (
                "lexical-v2",
                "cat >/dev/null; printf '%s\\n' '{\"protocol\":2,\"kind\":\"search\",\"response\":{\"query\":\"query\",\"realized_mode\":\"lexical\",\"results\":[]}}'\n",
                Some("incompatible-response"),
            ),
            (
                "unready-v2",
                "cat >/dev/null; printf 'SSH host banner\\n%s\\n' '{\"error\":{\"kind\":\"search-not-ready\",\"message\":\"run index\"}}' >&2; exit 8\n",
                Some("search-not-ready"),
            ),
            (
                "hybrid-v2",
                "cat >/dev/null; printf '%s\\n' '{\"protocol\":2,\"kind\":\"search\",\"response\":{\"query\":\"query\",\"realized_mode\":\"hybrid\",\"results\":[]}}'\n",
                None,
            ),
        ] {
            let executable = fake_executable(directory.path(), name, body);
            let result = remote_search_with(
                executable.as_os_str(),
                name.to_owned(),
                &request,
                Duration::from_secs(1),
            );
            assert_eq!(result.outcome.error_kind.as_deref(), expected, "{name}");
            if expected.is_none() {
                assert!(result.response.is_some());
                assert_eq!(result.outcome.realized_mode.as_deref(), Some("hybrid"));
            } else {
                assert!(result.response.is_none());
            }
        }
    }

    #[test]
    fn rank_merge_deduplicates_by_identity_without_score_inflation() {
        let local = response(vec![hit("shared", "conversation", "local copy")]);
        let remote = RemoteResult {
            response: Some(response(vec![
                hit("shared", "conversation", "remote copy"),
                hit("remote", "other", "remote only"),
            ])),
            outcome: NodeOutcome {
                node: "dev-macbook".to_owned(),
                status: "ok".to_owned(),
                elapsed_ms: 1,
                realized_mode: None,
                error_kind: None,
                error: None,
            },
        };
        let merged = merge_search(local, vec![remote], 10);
        assert_eq!(merged.results.len(), 2);
        assert_eq!(merged.results[0].id, "shared");
        assert_eq!(merged.results[0].federated_score, Some(1.0));
        assert_eq!(merged.results[0].origins, ["local", "dev-macbook"]);
        assert_eq!(merged.results[1].federated_score, Some(0.5));
    }

    #[test]
    fn rank_merge_breaks_equal_scores_by_identity() {
        let mut local_hit = hit("z-message", "conversation", "local");
        local_hit.provider = "zeta".to_owned();
        let mut remote_hit = hit("a-message", "conversation", "remote");
        remote_hit.provider = "alpha".to_owned();
        let remote = RemoteResult {
            response: Some(response(vec![remote_hit])),
            outcome: NodeOutcome {
                node: "node".to_owned(),
                status: "ok".to_owned(),
                elapsed_ms: 1,
                realized_mode: None,
                error_kind: None,
                error: None,
            },
        };
        let merged = merge_search(response(vec![local_hit]), vec![remote], 10);
        assert_eq!(merged.results[0].provider, "alpha");
        assert_eq!(merged.results[1].provider, "zeta");
    }

    #[test]
    fn rank_merge_preserves_ready_results_and_unready_node_outcomes() {
        let ready = RemoteResult {
            response: Some(response(vec![hit(
                "remote",
                "remote-conversation",
                "ready",
            )])),
            outcome: NodeOutcome {
                node: "ready-node".to_owned(),
                status: "ok".to_owned(),
                elapsed_ms: 1,
                realized_mode: Some("hybrid".to_owned()),
                error_kind: None,
                error: None,
            },
        };
        let unready = remote_error(
            "unready-node".to_owned(),
            2,
            "search-not-ready",
            "run index".to_owned(),
        );

        let merged = merge_search(
            response(vec![hit("local", "local-conversation", "local")]),
            vec![ready, unready],
            10,
        );
        assert_eq!(merged.realized_mode, "federated");
        assert_eq!(merged.results.len(), 2);
        let outcomes = merged.nodes.expect("node outcomes");
        assert_eq!(outcomes.len(), 2);
        assert_eq!(outcomes[0].realized_mode.as_deref(), Some("hybrid"));
        assert_eq!(outcomes[1].error_kind.as_deref(), Some("search-not-ready"));
    }

    fn response(results: Vec<SearchHit>) -> SearchResponse {
        SearchResponse {
            query: "query".to_owned(),
            realized_mode: "hybrid".to_owned(),
            results,
            nodes: None,
        }
    }

    fn hit(id: &str, conversation_id: &str, content: &str) -> SearchHit {
        SearchHit {
            id: id.to_owned(),
            conversation_id: conversation_id.to_owned(),
            provider: "claude-code".to_owned(),
            role: "user".to_owned(),
            content: content.to_owned(),
            lexical_score: None,
            semantic_score: None,
            fusion_score: 0.0,
            rerank_score: None,
            origins: Vec::new(),
            federated_score: None,
        }
    }

    fn fake_executable(directory: &Path, name: &str, body: &str) -> std::path::PathBuf {
        let path = directory.join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}")).expect("fake executable");
        let mut permissions = fs::metadata(&path).expect("metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).expect("executable permissions");
        path
    }
}
