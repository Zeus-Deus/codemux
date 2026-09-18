fn main() {
    println!("{}", serde_json::to_string_pretty(&schemars::schema_for!(codemux_addon_protocol::catalog::Catalog)).unwrap());
}
