use schemars::schema_for;
use shardrail_protocol::ContractSchema;
use std::{fs, path::PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../schema");
    fs::create_dir_all(&root)?;
    fs::write(
        root.join("contracts.schema.json"),
        serde_json::to_string_pretty(&schema_for!(ContractSchema))? + "\n",
    )?;
    Ok(())
}
