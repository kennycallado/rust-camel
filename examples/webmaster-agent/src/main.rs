use std::error::Error;

use camel_agent::{build_system_snapshot, parse_routes_yaml, MaintainerAgent};

fn main() -> Result<(), Box<dyn Error>> {
    let routes_file = parse_routes_yaml(include_str!("../routes.yaml"))?;
    let snapshot = build_system_snapshot(&routes_file.routes);
    let proposals = MaintainerAgent.analyze(&snapshot);

    println!("=== Camel Webmaster Agent POC ===");
    println!("Snapshot:\n{}", serde_json::to_string_pretty(&snapshot)?);
    println!(
        "\nMaintenance proposals ({}):\n{}",
        proposals.len(),
        serde_json::to_string_pretty(&proposals)?
    );

    if proposals.len() < 3 {
        return Err("expected at least 3 maintenance proposals".into());
    }

    Ok(())
}
