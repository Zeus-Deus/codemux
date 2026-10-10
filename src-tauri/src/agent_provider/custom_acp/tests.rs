use super::catalog::*;
use serde_json::json;

#[test]
fn installed_dsh_acp_v1_specimen_preserves_json_encoded_model_and_resume_only() {
    // Exact public initialize/configOptions projection of parent's credential-free
    // installed DSH probe, not a reconstructed vendor example.
    let specimen: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/dsh_acp_v1_catalog.json"
    ))
    .unwrap();
    let caps = Negotiation::parse(&specimen["initialize"]).unwrap();
    assert_eq!(caps.resume_method().unwrap(), "session/resume");
    assert!(!caps.load);
    assert!(caps.close);
    assert!(caps.auth_methods.is_empty());
    let catalog = catalog_from(
        "dsh",
        "DSH",
        &caps,
        &json!({"configOptions":specimen["configOptions"]}),
    )
    .unwrap();
    let model = "[\"deepseek-official\",\"deepseek-v4-flash\"]";
    assert_eq!(catalog.current_model.as_deref(), Some(model));
    assert_eq!(catalog.capabilities.models[0].id, model);
    assert_eq!(catalog.capabilities.models.len(), 3);
    assert_eq!(
        catalog.capabilities.models[0].effort_levels,
        vec!["off", "low", "high", "max"]
    );
    assert_eq!(
        catalog.capabilities.models[0].default_effort.as_deref(),
        Some("high")
    );
    assert!(!catalog.supports_images);
}

#[test]
fn catalogs_use_only_advertised_models_and_effort() {
    let caps = Negotiation::parse(&json!({"protocolVersion":1})).unwrap();
    let empty = catalog_from("a", "A", &caps, &json!({})).unwrap();
    assert!(empty.capabilities.models.is_empty());
    assert_eq!(empty.current_model, None);
    let catalog = catalog_from("a","A",&caps,&json!({"configOptions":[
        {"id":"MODEL-ID","name":"Model","category":"model","type":"select","currentValue":"default","options":[{"value":"default","name":"Real Default"},{"value":"vendor:model [1m]","name":"Opaque"}]},
        {"id":"Think-ID","name":"Effort","category":"thought_level","type":"select","currentValue":" Big ","options":[{"value":" Big ","name":"Big"}]}
    ]})).unwrap();
    assert_eq!(catalog.capabilities.models[0].id, "default");
    assert_eq!(catalog.capabilities.models[1].id, "vendor:model [1m]");
    assert_eq!(catalog.capabilities.models[0].effort_levels, vec![" Big "]);
    assert!(
        catalog.capabilities.models[1].effort_levels.is_empty(),
        "model-dependent effort must not be guessed for inactive models"
    );
    assert_eq!(
        catalog.capabilities.default_permission_mode.as_deref(),
        Some("supervised")
    );
}

#[test]
fn native_id_collision_is_scoped_by_instance_and_revision() {
    let a = namespaced_session("instance-a", "rev", "same native ID");
    assert_ne!(a, namespaced_session("instance-b", "rev", "same native ID"));
    assert_ne!(
        a,
        namespaced_session("instance-a", "next rev", "same native ID")
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&a.0).unwrap(),
        json!(["acp", "instance-a", "rev", "same native ID"])
    );
}

#[test]
fn config_values_validate_without_coercion() {
    let options = parse_options(&json!([
        {"id":"m","name":"Model","type":"select","currentValue":"default","options":[{"value":"default","name":"Default"}]},
        {"id":"b","name":"Flag","type":"boolean","currentValue":false},
        {"id":"ignored","type":"number"}
    ])).unwrap();
    assert_eq!(options.len(), 2);
    assert!(options[1].options.is_empty());
    assert!(validate_value(&options[0], &json!("Default")).is_err());
    assert!(validate_value(&options[0], &json!("default")).is_ok());
    assert!(validate_value(&options[1], &json!("true")).is_err());
    assert!(validate_value(&options[1], &json!(true)).is_ok());
}

#[test]
fn permission_decisions_validate_original_options_and_prefer_once() {
    assert!(
        permission_outcome(
            &[json!({"optionId":"sticky","kind":"allow_always"})],
            &crate::agent_provider::ApprovalDecision::AllowForSession
        )
        .is_err(),
        "ACP allow_always must not be misrepresented as session-scoped host approval"
    );
    use crate::agent_provider::ApprovalDecision;
    let options = vec![
        json!({"optionId":"always ID", "kind":"allow_always"}),
        json!({"optionId":"once ID ","kind":"allow_once"}),
    ];
    assert_eq!(
        permission_outcome(
            &options,
            &ApprovalDecision::Allow {
                updated_input: None,
                updated_permissions: None
            }
        )
        .unwrap(),
        json!({"outcome":{"outcome":"selected","optionId":"once ID "}})
    );
    assert!(permission_outcome(
        &options,
        &ApprovalDecision::ProviderOption {
            option_id: "once id".into()
        }
    )
    .is_err());
    assert_eq!(
        permission_outcome(&options, &ApprovalDecision::Cancel).unwrap(),
        json!({"outcome":{"outcome":"cancelled"}})
    );
}

#[test]
fn acp_v1_resume_only_and_version_validation() {
    let caps = Negotiation::parse(&json!({"protocolVersion":1,"agentCapabilities":{
        "loadSession":false,"sessionCapabilities":{"resume":{},"close":{}}
    },"authMethods":[]}))
    .unwrap();
    assert_eq!(caps.resume_method().unwrap(), "session/resume");
    assert!(caps.close);
    assert!(!caps.images);
    for bad in [
        json!({}),
        json!({"protocolVersion":2}),
        json!({"protocolVersion":"1"}),
    ] {
        assert!(Negotiation::parse(&bad).is_err());
    }
    let bad = Negotiation::parse(
        &json!({"protocolVersion":1,"agentCapabilities":{"sessionCapabilities":{"resume":true}}}),
    );
    assert!(bad.is_err());
}

#[test]
fn grouped_model_values_remain_opaque() {
    let parsed = parse_options(&json!([{
        "id":" Model/ID ", "name":"Models", "category":"model", "type":"select",
        "currentValue":"vendor:model [1m]", "options":[{"group":"provider", "name":"Provider",
            "options":[{"value":"vendor:model [1m]", "name":"Opaque"},
                       {"value":"default", "name":"Literal default"}]}]
    }]))
    .unwrap();
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].id, " Model/ID ");
    assert_eq!(parsed[0].current_value, json!("vendor:model [1m]"));
    assert_eq!(parsed[0].options[0].value, "vendor:model [1m]");
    assert_eq!(parsed[0].options[1].value, "default");
}
