use super::hash_entries::{completion_hash_entries, model_hash_entries, stage_hash_entries};
use super::hash_projection;
use super::*;
use crate::content::{ContentBlock, ToolCallBlock};
use crate::reducer::KernelError;

mod validate_tool_state_tests {
    use std::sync::Arc;

    use super::*;
    use crate::conversation::{Message, MessageRole, ProviderIds};
    use crate::primitives::MessageId;
    use crate::primitives::{Metadata, RawJson};
    use crate::tools::ToolCallIdentity;

    /// Reference implementation of the authorship check: the nested scan the
    /// indexed version replaced. Any input the two disagree on is a regression
    /// in a fail-closed boundary, so the equivalence is asserted directly.
    fn authored_by_nested_scan(state: &KernelState, identity: &ToolCallIdentity) -> bool {
        state.messages.iter().any(|message| {
            *message.id() == identity.source_message_id
                && message.role() == MessageRole::Assistant
                && message.content().iter().any(
                    |block| matches!(block, ContentBlock::ToolCall(call) if call == &identity.call),
                )
        })
    }

    fn id<T: crate::IdTag>(ordinal: u64) -> crate::Id<T> {
        let mut bytes = [0_u8; 16];
        bytes[6] = 0x70;
        bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        crate::Id::from_bytes(bytes)
    }

    fn tool_call(ordinal: u64, name: &str) -> ToolCallBlock {
        ToolCallBlock::try_new(
            id::<crate::ToolCallTag>(ordinal),
            name,
            RawJson::parse(format!(r#"{{"ordinal":{ordinal}}}"#)).expect("arguments"),
        )
        .expect("tool call")
    }

    fn message(message_id: MessageId, role: MessageRole, blocks: Vec<ContentBlock>) -> Message {
        Message::try_new(
            message_id,
            role,
            blocks,
            crate::Timestamp::from_unix_ms(1_000).expect("timestamp"),
            None,
            ProviderIds::empty(),
            Metadata::empty(),
        )
        .expect("message")
    }

    /// Builds a state whose single tool call is authored by its source message.
    fn authored_state() -> (KernelState, ToolCallIdentity) {
        let source_message_id = id::<crate::MessageTag>(1);
        let call = tool_call(2, "lookup_price");
        let identity = ToolCallIdentity {
            cycle: 0,
            turn_id: id::<crate::TurnTag>(3),
            source_message_id,
            tool_batch_id: None,
            effect_id: None,
            call: call.clone(),
        };
        let state = KernelState {
            state_version: 2,
            messages: Arc::new(vec![message(
                source_message_id,
                MessageRole::Assistant,
                vec![ContentBlock::ToolCall(call.clone())],
            )]),
            tool_calls: [(*call.tool_call_id(), identity.clone())]
                .into_iter()
                .collect(),
            ..KernelState::default()
        };
        (state, identity)
    }

    #[test]
    fn indexed_authorship_matches_the_nested_scan_across_mutations() {
        let (base, identity) = authored_state();

        // Authored: both forms agree it is present.
        assert!(authored_by_nested_scan(&base, &identity));
        assert_eq!(base.validate_tool_state(), Ok(()));

        // Wrong source message id.
        let mut wrong_source = identity.clone();
        wrong_source.source_message_id = id::<crate::MessageTag>(99);
        let mut state = base.clone();
        state.tool_calls = [(*wrong_source.call.tool_call_id(), wrong_source.clone())]
            .into_iter()
            .collect();
        assert!(!authored_by_nested_scan(&state, &wrong_source));
        assert!(state.validate_tool_state().is_err());

        // Author message exists but carries a different call payload.
        let mut different_args = base.clone();
        different_args.messages = Arc::new(vec![message(
            identity.source_message_id,
            MessageRole::Assistant,
            vec![ContentBlock::ToolCall(tool_call(2, "different_tool"))],
        )]);
        assert!(!authored_by_nested_scan(&different_args, &identity));
        assert!(different_args.validate_tool_state().is_err());

        // A non-assistant author is unrepresentable: `Message::try_new` rejects
        // a tool-role message carrying a tool-call block, so the role filter in
        // both forms can only ever see assistant authorship.
        assert!(
            Message::try_new(
                identity.source_message_id,
                MessageRole::Tool,
                vec![ContentBlock::ToolCall(identity.call.clone())],
                crate::Timestamp::from_unix_ms(1_000).expect("timestamp"),
                None,
                ProviderIds::empty(),
                Metadata::empty(),
            )
            .is_err()
        );

        // Same id authored by a non-source message must not satisfy the check.
        let mut other_author = base.clone();
        other_author.messages = Arc::new(vec![message(
            id::<crate::MessageTag>(42),
            MessageRole::Assistant,
            vec![ContentBlock::ToolCall(identity.call.clone())],
        )]);
        assert!(!authored_by_nested_scan(&other_author, &identity));
        assert!(other_author.validate_tool_state().is_err());
    }

    #[test]
    fn duplicate_effect_assignments_are_still_rejected() {
        let (mut state, identity) = authored_state();
        let second_call = tool_call(7, "lookup_quote");
        let effect_id = id::<crate::EffectTag>(11);
        let batch_id = id::<crate::ToolBatchTag>(12);

        let mut first = identity.clone();
        first.effect_id = Some(effect_id);
        first.tool_batch_id = Some(batch_id);
        let mut second = ToolCallIdentity {
            call: second_call.clone(),
            ..identity.clone()
        };
        second.effect_id = Some(effect_id);
        second.tool_batch_id = Some(batch_id);

        state.messages = Arc::new(vec![message(
            identity.source_message_id,
            MessageRole::Assistant,
            vec![
                ContentBlock::ToolCall(identity.call.clone()),
                ContentBlock::ToolCall(second_call.clone()),
            ],
        )]);
        state.tool_calls = [
            (*first.call.tool_call_id(), first),
            (*second.call.tool_call_id(), second),
        ]
        .into_iter()
        .collect();

        assert_eq!(
            state.validate_tool_state(),
            Err(KernelError::InvalidInputPayload {
                field: "tool_state",
                reason_code: "inconsistent",
            }),
            "the same effect id assigned to two calls must stay rejected"
        );
    }
}

mod kernel_state_eq_tests {
    use super::*;

    #[test]
    fn eq_is_wire_json_not_state_hash_projection() {
        let left = KernelState::default();
        let right = KernelState::default();
        let left_wire = serde_json_canonicalizer::to_vec(&left).expect("left wire");
        let right_wire = serde_json_canonicalizer::to_vec(&right).expect("right wire");
        assert_eq!(left_wire, right_wire);
        assert_eq!(left, right);

        let hash_projection = hash_projection::KernelStateHashV1::from_state(
            &left,
            stage_hash_entries(&left.stage_settlements),
            model_hash_entries(&left.model_settlements),
            completion_hash_entries(&left.completion_identities),
        );
        let hash_json =
            serde_json_canonicalizer::to_vec(&hash_projection).expect("hash projection");
        assert_ne!(
            left_wire, hash_json,
            "wire Serialize and hash projection must stay distinct"
        );

        let mut writer = crate::primitives::DigestWriter::new("kernel-state", 1).expect("writer");
        serde_json_canonicalizer::to_writer(&hash_projection, &mut writer).expect("hash write");
        let (digest, _) = writer.finish();
        assert_eq!(left.state_hash().expect("hash"), digest);
        assert_eq!(
            left.state_hash().expect("hash"),
            right.state_hash().expect("hash")
        );
    }
}
