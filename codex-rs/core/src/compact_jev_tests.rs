use super::*;
use codex_protocol::models::FunctionCallOutputPayload;
use pretty_assertions::assert_eq;

fn call(call_id: &str) -> ResponseItemEnvelope {
    ResponseItemEnvelope::new(ResponseItem::FunctionCall {
        id: None,
        name: "shell".to_string(),
        namespace: None,
        arguments: "{}".to_string(),
        encrypted_function_args: None,
        call_id: call_id.to_string(),
        internal_chat_message_metadata_passthrough: None,
    })
}

fn output(call_id: &str, text: &str) -> ResponseItemEnvelope {
    ResponseItemEnvelope::new(ResponseItem::FunctionCallOutput {
        id: None,
        call_id: Some(call_id.to_string()),
        name: None,
        namespace: None,
        output: FunctionCallOutputPayload::from_text(text.to_string()),
        internal_chat_message_metadata_passthrough: None,
    })
}

fn answer(keep: &[usize], replace: &[(usize, &str)]) -> HelperAnswer {
    HelperAnswer {
        keep: keep.to_vec(),
        replace: replace
            .iter()
            .map(|(index, text)| (index.to_string(), text.to_string()))
            .collect(),
    }
}

#[test]
fn drops_pairs_and_truncates_outputs() {
    let items = vec![
        call("a"),
        output("a", "long a"),
        call("b"),
        output("b", "long b"),
    ];

    let pruned = apply_answer(&items, answer(&[2, 3], &[(3, "short b")]));

    assert_eq!(pruned, Some(vec![call("b"), output("b", "short b")]));
}

#[test]
fn rejects_answers_that_orphan_or_reorder() {
    let items = vec![call("a"), output("a", "long a")];

    assert_eq!(apply_answer(&items, answer(&[1], &[])), None);
    assert_eq!(apply_answer(&items, answer(&[1, 0], &[])), None);
    assert_eq!(apply_answer(&items, answer(&[0, 1, 2], &[])), None);
    assert_eq!(
        apply_answer(&items, answer(&[0, 1], &[(0, "not an output")])),
        None
    );
}
