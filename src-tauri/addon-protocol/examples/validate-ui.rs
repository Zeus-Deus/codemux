use codemux_addon_protocol::ui::Tree;
fn main() {
 let mut tree=Tree::default();
 let batches:Vec<Vec<serde_json::Value>>=serde_json::from_reader(std::io::stdin()).unwrap();
 for (index,batch) in batches.iter().enumerate() { if let Err(error)=tree.apply(batch) { eprintln!("Batch {index}: {}\n{}",error.message,serde_json::to_string_pretty(batch).unwrap());std::process::exit(1) } }
 println!("Valid UI tree");
}
