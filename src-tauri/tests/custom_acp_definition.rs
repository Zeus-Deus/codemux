use codemux_lib::agent_provider::ProviderKind;

#[test]
fn acp_provider_kind_roundtrips_without_relabeling_a_native_provider() {
    let result = serde_json::from_value::<ProviderKind>(serde_json::json!("acp"));
    assert!(result.is_ok(), "generic ACP is a distinct serialized provider family");
    assert_eq!(serde_json::to_value(result.unwrap()).unwrap(), "acp");
}
