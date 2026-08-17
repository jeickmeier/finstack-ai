#[tokio::test]
async fn parent_chain_is_immutable_and_equal_replay_is_idempotent() {
    let store = store();
    let mut coordinator = CommitCoordinator::new(store);
    bootstrap(&mut coordinator, 1, 2, 10).await;
    let user = coordinator
        .session()
        .entries()
        .values()
        .next()
        .expect("user")
        .clone();
    coordinator
        .commit_session_records(
            id(510),
            vec![session_draft(
                511,
                1,
                2,
                RecordBody::ConversationEntry(user.clone()),
            )],
        )
        .await
        .expect("equal replay");
    let rewritten = ConversationEntry::try_new(
        user.id(),
        Some(id(99)),
        user.lane_id(),
        0,
        user.body().clone(),
    )
    .expect("rewrite");
    assert!(matches!(
        coordinator
            .commit_session_records(
                id(512),
                vec![session_draft(
                    513,
                    1,
                    2,
                    RecordBody::ConversationEntry(rewritten)
                )],
            )
            .await,
        Err(CommitCoordinatorError::BoundaryFault {
            code: "session_records_invalid"
        })
    ));
    assert_eq!(
        coordinator
            .session()
            .entries()
            .get(&user.id())
            .expect("kept")
            .parent_id(),
        None
    );
}

#[tokio::test]
async fn branch_foundation_does_not_rewrite_the_shared_parent() {
    let store = store();
    let mut coordinator = CommitCoordinator::new(store);
    bootstrap(&mut coordinator, 1, 2, 10).await;
    let a = coordinator
        .session()
        .main_lane()
        .expect("main")
        .1
        .leaf_id
        .expect("leaf");
    let b_msg = text_message(11, MessageRole::Assistant, "b");
    let c_msg = text_message(12, MessageRole::User, "c");
    let d_msg = text_message(13, MessageRole::User, "d");
    let b = ConversationEntry::from_message(&b_msg, Some(a), id(2), 0).expect("b");
    let c = ConversationEntry::from_message(&c_msg, Some(b.id()), id(2), 0).expect("c");
    let d = ConversationEntry::from_message(&d_msg, Some(b.id()), id(2), 0).expect("d");
    coordinator
        .commit_session_records(
            id(520),
            vec![
                session_draft(521, 1, 2, RecordBody::ConversationEntry(b.clone())),
                session_draft(522, 1, 2, RecordBody::LaneMoved(LaneMoved::new(b.id()))),
            ],
        )
        .await
        .expect("b");
    coordinator
        .commit_session_records(
            id(523),
            vec![
                session_draft(524, 1, 2, RecordBody::ConversationEntry(c.clone())),
                session_draft(525, 1, 2, RecordBody::LaneMoved(LaneMoved::new(c.id()))),
            ],
        )
        .await
        .expect("c");
    coordinator
        .commit_session_records(
            id(526),
            vec![
                session_draft(527, 1, 2, RecordBody::ConversationEntry(d.clone())),
                session_draft(528, 1, 2, RecordBody::LaneMoved(LaneMoved::new(d.id()))),
            ],
        )
        .await
        .expect("d");
    assert_eq!(
        coordinator
            .session()
            .entries()
            .get(&b.id())
            .expect("b")
            .parent_id(),
        Some(a)
    );
    let history_d = coordinator.session().history(d.id()).expect("d");
    let history_c = coordinator.session().history(c.id()).expect("c");
    assert_eq!(
        history_d
            .iter()
            .map(ConversationEntry::id)
            .collect::<Vec<_>>(),
        vec![a, b.id(), d.id()]
    );
    assert_eq!(
        history_c
            .iter()
            .map(ConversationEntry::id)
            .collect::<Vec<_>>(),
        vec![a, b.id(), c.id()]
    );
}

#[tokio::test]
async fn extract_history_keeps_tool_pairs_and_ignores_compaction() {
    let store = store();
    let mut coordinator = CommitCoordinator::new(store);
    bootstrap(&mut coordinator, 1, 2, 10).await;
    let user = coordinator
        .session()
        .main_lane()
        .expect("main")
        .1
        .leaf_id
        .expect("user");
    let (assistant_entry, tool_entry) = closed_tool_pair(user);
    coordinator
        .commit_session_records(
            id(530),
            vec![
                session_draft(
                    531,
                    1,
                    2,
                    RecordBody::ConversationEntry(assistant_entry.clone()),
                ),
                session_draft(532, 1, 2, RecordBody::ConversationEntry(tool_entry.clone())),
                session_draft(
                    533,
                    1,
                    2,
                    RecordBody::LaneMoved(LaneMoved::new(tool_entry.id())),
                ),
            ],
        )
        .await
        .expect("pair");
    let history = coordinator
        .session()
        .history(tool_entry.id())
        .expect("history");
    assert_eq!(history.len(), 3);
    let canonical = serde_json_canonicalizer::to_vec(&Vec::<Message>::new()).expect("json");
    let digest = Digest::domain_separated("model-context", 1, &canonical).expect("digest");
    let compacted = ContextPrepared::try_new(0, id(30), Vec::new(), digest).expect("context");
    let envelope = RecordEnvelope::try_new(
        RECORD_FORMAT_VERSION,
        RECORD_KIND_VERSION,
        id(540),
        id(1),
        id(2),
        Some(id(3)),
        99,
        timestamp(0),
        None,
        Digest::raw_json(b"p"),
        None,
        Digest::raw_json(b"c"),
        Vec::new(),
        RecordBody::ContextPrepared(compacted),
    )
    .expect("envelope");
    let mut projection = coordinator.session().clone();
    projection.apply_envelope(&envelope).expect("ignore");
    assert_eq!(projection.history(tool_entry.id()).expect("still"), history);
    let split = assistant_entry.clone();
    let mut incomplete = BTreeMap::new();
    incomplete.insert(user, coordinator.session().entries()[&user].clone());
    incomplete.insert(split.id(), split.clone());
    assert_eq!(
        finstack_ai_kernel::extract_history(&incomplete, split.id()),
        Err(ConversationError::InvalidToolPair)
    );
}

#[test]
fn conversation_entry_is_structural_and_emits_zero_events() {
    let message = text_message(10, MessageRole::User, "hello");
    let entry = ConversationEntry::from_message(&message, None, id(2), 1).expect("entry");
    let body = RecordBody::ConversationEntry(entry);
    assert_eq!(body.kind_name(), "conversation_entry");
    assert!(body.is_structural());
    assert_eq!(
        body.derived_event_count(RECORD_KIND_VERSION)
            .expect("count"),
        0
    );
}
