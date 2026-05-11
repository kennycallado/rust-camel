use std::error::Error;

use camel_agent::{MaintainerAgent, build_system_snapshot_from_yaml};

fn main() -> Result<(), Box<dyn Error>> {
    let snapshot = build_system_snapshot_from_yaml(include_str!("../routes.yaml"))?;
    let proposals = MaintainerAgent.analyze(&snapshot);

    println!("=== Camel Webmaster Agent POC ===");
    println!("Snapshot:\n{}", serde_json::to_string_pretty(&snapshot)?);
    println!(
        "\nMaintenance proposals ({}):\n{}",
        proposals.len(),
        serde_json::to_string_pretty(&proposals)?
    );

    Ok(())
}
