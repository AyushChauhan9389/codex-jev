//! Jev-guided verbatim pruning, tried before a summarizing compaction.
//!
//! Opt-in through `CODEX_JEV_COMPACT`, the path of an executable helper. The helper reads the
//! history as a JSON array of response items on stdin and answers on stdout with
//! `{"keep": [index, ...], "replace": {"index": "text"}}`: the items to keep, in order, and new
//! text for tool outputs it truncated. It exits non-zero when pruning is not worth it. Items are
//! rebuilt from the originals here, so nothing but the replaced output text is ever rewritten;
//! any error, timeout or answer that would leave a call without its output falls back to the
//! regular compaction.

use std::collections::HashMap;
use std::collections::HashSet;
use std::process::Stdio;
use std::time::Duration;

use codex_history::ResponseItemEnvelope;
use codex_protocol::models::FunctionCallOutputBody;
use codex_protocol::models::ResponseItem;
use serde::Deserialize;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tracing::info;
use tracing::warn;

const HELPER_ENV_VAR: &str = "CODEX_JEV_COMPACT";
// ponytail: fixed timeout; make it configurable if Jev latency ever needs more.
const HELPER_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Deserialize)]
struct HelperAnswer {
    keep: Vec<usize>,
    #[serde(default)]
    replace: HashMap<String, String>,
}

/// Returns the pruned history, or `None` when pruning is off, fails or is not worth it.
pub(crate) async fn prune_history(
    items: &[ResponseItemEnvelope],
) -> Option<Vec<ResponseItemEnvelope>> {
    let helper = std::env::var_os(HELPER_ENV_VAR)?;
    let raw_items: Vec<&ResponseItem> = items.iter().map(|envelope| &envelope.item).collect();
    let input = match serde_json::to_vec(&raw_items) {
        Ok(input) => input,
        Err(err) => {
            warn!("jev compaction: failed to serialize history: {err}");
            return None;
        }
    };
    let answer = match tokio::time::timeout(HELPER_TIMEOUT, run_helper(&helper, input)).await {
        Ok(Ok(answer)) => answer,
        Ok(Err(err)) => {
            info!("jev compaction skipped: {err}");
            return None;
        }
        Err(_) => {
            warn!("jev compaction: helper timed out");
            return None;
        }
    };
    let pruned = apply_answer(items, answer);
    if pruned.is_none() {
        warn!("jev compaction: helper answer rejected (would orphan a call or output)");
    }
    pruned
}

async fn run_helper(helper: &std::ffi::OsStr, input: Vec<u8>) -> Result<HelperAnswer, String> {
    let mut child = Command::new(helper)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|err| format!("failed to start helper: {err}"))?;
    let mut stdin = child.stdin.take().ok_or("helper stdin unavailable")?;
    stdin
        .write_all(&input)
        .await
        .map_err(|err| format!("failed to write history: {err}"))?;
    drop(stdin);
    let output = child
        .wait_with_output()
        .await
        .map_err(|err| format!("helper failed: {err}"))?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        return Err(format!(
            "helper exited with {}: {}",
            output.status,
            stderr.trim()
        ));
    }
    info!("jev compaction: {}", stderr.trim());
    serde_json::from_slice(&output.stdout).map_err(|err| format!("invalid helper answer: {err}"))
}

fn apply_answer(
    items: &[ResponseItemEnvelope],
    answer: HelperAnswer,
) -> Option<Vec<ResponseItemEnvelope>> {
    if answer.keep.windows(2).any(|pair| pair[0] >= pair[1])
        || answer.keep.last().is_some_and(|&last| last >= items.len())
    {
        return None;
    }
    let mut pruned = Vec::with_capacity(answer.keep.len());
    for &index in &answer.keep {
        let mut envelope = items[index].clone();
        if let Some(text) = answer.replace.get(&index.to_string()) {
            match &mut envelope.item {
                ResponseItem::FunctionCallOutput { output, .. }
                | ResponseItem::CustomToolCallOutput { output, .. } => {
                    output.body = FunctionCallOutputBody::Text(text.clone());
                }
                _ => return None,
            }
        }
        pruned.push(envelope);
    }
    // Every call id must keep both halves or lose both: the Responses API rejects an output
    // without its call, and a call without its output.
    let kept: HashSet<usize> = answer.keep.iter().copied().collect();
    let mut halves: HashMap<&str, (usize, usize)> = HashMap::new();
    for (index, envelope) in items.iter().enumerate() {
        let Some(call_id) = call_id_of(&envelope.item) else {
            continue;
        };
        let (total, retained) = halves.entry(call_id).or_default();
        *total += 1;
        if kept.contains(&index) {
            *retained += 1;
        }
    }
    halves
        .values()
        .all(|&(total, retained)| retained == 0 || retained == total)
        .then_some(pruned)
}

/// The call id of a tool call or tool output item.
fn call_id_of(item: &ResponseItem) -> Option<&str> {
    match item {
        ResponseItem::FunctionCall { call_id, .. }
        | ResponseItem::CustomToolCall { call_id, .. }
        | ResponseItem::CustomToolCallOutput { call_id, .. } => Some(call_id),
        ResponseItem::LocalShellCall { call_id, .. }
        | ResponseItem::ToolSearchCall { call_id, .. }
        | ResponseItem::FunctionCallOutput { call_id, .. }
        | ResponseItem::ToolSearchOutput { call_id, .. } => call_id.as_deref(),
        _ => None,
    }
}

#[cfg(test)]
#[path = "compact_jev_tests.rs"]
mod tests;
