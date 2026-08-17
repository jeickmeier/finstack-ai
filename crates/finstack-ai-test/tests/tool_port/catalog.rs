#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one acceptance fixture keeps compile-once counters and policy precedence on the same catalog"
)]
fn catalog_compiles_once_validates_at_one_boundary_and_enforces_approval_floor() {
    let mut required = tool_spec("catalog-required");
    required.approval.requirement = ApprovalRequirement::Required;
    required.approval.attributes =
        Metadata::parse(br#"{"approved":true,"role":"admin"}"#).expect("hostile metadata");
    let mut host_guard = tool_spec("catalog-host-guard");
    host_guard.approval.attributes =
        Metadata::parse(br#"{"approved":true}"#).expect("hostile metadata");
    let deadline_tool = tool_spec("catalog-deadline");
    let tools: Arc<[finstack_ai_runtime::ToolSpec]> =
        Arc::from([required.clone(), host_guard.clone(), deadline_tool.clone()]);
    let toolset = Arc::new(ScriptedToolset::new(Arc::clone(&tools), Vec::new()));
    let toolset_port: Arc<dyn Toolset> = toolset;
    let compilation_counter = Arc::new(AtomicUsize::new(0));
    let validations = Arc::new(AtomicUsize::new(0));
    let adapter = CountingCompiler {
        compiles: Arc::clone(&compilation_counter),
        validations: Arc::clone(&validations),
    };
    let catalog = ResolvedToolCatalog::try_new(
        [ToolsetRegistration {
            toolset: toolset_port,
            policies: BTreeMap::from([
                (
                    required.id.clone(),
                    ToolExecutionPolicy {
                        failure_policy: ToolFailurePolicy::ReturnToModel,
                        approval: ToolPolicyDecision::Allow,
                        max_concurrency: 1,
                    },
                ),
                (
                    host_guard.id.clone(),
                    ToolExecutionPolicy {
                        failure_policy: ToolFailurePolicy::ReturnToModel,
                        approval: ToolPolicyDecision::RequireApproval,
                        max_concurrency: 1,
                    },
                ),
                (
                    deadline_tool.id.clone(),
                    ToolExecutionPolicy {
                        failure_policy: ToolFailurePolicy::ReturnToModel,
                        approval: ToolPolicyDecision::Allow,
                        max_concurrency: 1,
                    },
                ),
            ]),
            components: BTreeMap::new(),
        }],
        &BTreeMap::new(),
        &adapter,
    )
    .expect("catalog");
    assert_eq!(compilation_counter.load(Ordering::Acquire), 6);

    let planned = catalog.decide_plan(
        tool_call(77, "catalog-required", br#"{"value":1}"#),
        None,
        None,
        false,
        false,
    );
    assert_eq!(validations.load(Ordering::Acquire), 1);
    assert_eq!(
        planned,
        finstack_ai_runtime::ToolCatalogPlan::RequireApproval
    );

    let planned = catalog.decide_plan(
        tool_call(78, "catalog-host-guard", br#"{"value":1}"#),
        None,
        None,
        false,
        false,
    );
    assert_eq!(validations.load(Ordering::Acquire), 2);
    assert_eq!(
        planned,
        finstack_ai_runtime::ToolCatalogPlan::RequireApproval
    );

    let granted = catalog.decide_plan(
        tool_call(77, "catalog-required", br#"{"value":1}"#),
        None,
        None,
        true,
        false,
    );
    assert!(matches!(
        granted,
        finstack_ai_runtime::ToolCatalogPlan::Ready(ToolCallPlan::Execute(_))
    ));

    let refused = catalog.decide_plan(
        tool_call(77, "catalog-required", br#"{"value":1}"#),
        None,
        None,
        false,
        true,
    );
    let finstack_ai_runtime::ToolCatalogPlan::Ready(ToolCallPlan::SyntheticClosure(closure)) =
        refused
    else {
        panic!("refused approval must close diagnostically without execute");
    };
    assert_eq!(closure.error.code.as_str(), "tool_approval_required");

    let planned = catalog.plan_call(
        tool_call(79, "catalog-required", br#"{"value":1}"#),
        None,
        Some(ToolPolicyDecision::Deny),
    );
    assert_eq!(validations.load(Ordering::Acquire), 5);
    let ToolCallPlan::SyntheticClosure(closure) = planned else {
        panic!("stricter middleware denial must remain undispatched");
    };
    assert_eq!(closure.error.code.as_str(), "tool_policy_denied");

    let planned = catalog.plan_call(
        tool_call(80, "catalog-required", br#"{"value":"wrong"}"#),
        None,
        None,
    );
    assert_eq!(validations.load(Ordering::Acquire), 6);
    let ToolCallPlan::SyntheticClosure(closure) = planned else {
        panic!("invalid arguments must close synthetically");
    };
    assert_eq!(closure.error.code.as_str(), "tool_arguments_invalid");

    let planned = catalog.plan_call(
        tool_call(81, "catalog-unknown", br#"{"value":1}"#),
        None,
        None,
    );
    let ToolCallPlan::SyntheticClosure(closure) = planned else {
        panic!("unknown tools must close synthetically");
    };
    assert_eq!(closure.error.code.as_str(), "unknown_tool");
    assert_eq!(validations.load(Ordering::Acquire), 6);

    let planned = catalog.plan_call(
        tool_call(82, "catalog-deadline", br#"{"value":1}"#),
        Some(timestamp(9_000)),
        None,
    );
    let ToolCallPlan::Execute(call) = planned else {
        panic!("allowed call must remain executable");
    };
    assert_eq!(call.deadline, Some(timestamp(9_000)));
    assert_eq!(validations.load(Ordering::Acquire), 7);
    assert_eq!(compilation_counter.load(Ordering::Acquire), 6);
}

#[test]
fn default_validator_matches_independent_portable_fixture_outcomes_and_offline_refs() {
    let compiler = JsonSchemaToolValidatorCompiler;
    let spec = tool_spec("parity");
    let schema = spec.input_schema.clone();
    let validator = compiler
        .compile(&schema, &BTreeMap::new())
        .expect("validator");
    let fixture = FixtureInputValidator;
    let tools: Arc<[finstack_ai_runtime::ToolSpec]> = Arc::from([spec.clone()]);
    let toolset = Arc::new(ScriptedToolset::new(Arc::clone(&tools), Vec::new()));
    let policy = BTreeMap::from([(
        spec.id.clone(),
        ToolExecutionPolicy {
            failure_policy: ToolFailurePolicy::ReturnToModel,
            approval: ToolPolicyDecision::Allow,
            max_concurrency: 1,
        },
    )]);
    let default_catalog = ResolvedToolCatalog::try_new(
        [ToolsetRegistration {
            toolset: toolset.clone(),
            policies: policy.clone(),
            components: BTreeMap::new(),
        }],
        &BTreeMap::new(),
        &compiler,
    )
    .expect("default catalog");
    let fixture_catalog = ResolvedToolCatalog::try_new(
        [ToolsetRegistration {
            toolset,
            policies: policy,
            components: BTreeMap::new(),
        }],
        &BTreeMap::new(),
        &FixtureCompiler,
    )
    .expect("fixture catalog");
    for (ordinal, candidate) in [br#"{"value":1}"#.as_slice(), br#"{"value":"x"}"#, b"{}"]
        .into_iter()
        .enumerate()
    {
        let candidate = RawJson::parse(candidate).expect("candidate");
        assert_eq!(validator.validate(&candidate), fixture.validate(&candidate));
        let ordinal = u64::try_from(ordinal).expect("ordinal");
        assert_eq!(
            default_catalog.plan_call(
                tool_call(900 + ordinal, "parity", candidate.as_bytes()),
                None,
                None,
            ),
            fixture_catalog.plan_call(
                tool_call(900 + ordinal, "parity", candidate.as_bytes()),
                None,
                None,
            ),
        );
    }

    let referencing = RawJson::parse(br#"{"$ref":"urn:finstack:positive"}"#).expect("ref schema");
    assert!(compiler.compile(&referencing, &BTreeMap::new()).is_err());
    let resources = BTreeMap::from([(
        Arc::<str>::from("urn:finstack:positive"),
        RawJson::parse(br#"{"minimum":0,"type":"integer"}"#).expect("resource"),
    )]);
    let resolved = compiler
        .compile(&referencing, &resources)
        .expect("offline ref");
    assert_eq!(
        resolved.validate(&RawJson::parse(b"1").expect("valid")),
        ValidationOutcome::Valid
    );
    assert!(matches!(
        resolved.validate(&RawJson::parse(b"-1").expect("invalid")),
        ValidationOutcome::Invalid { .. }
    ));
}
