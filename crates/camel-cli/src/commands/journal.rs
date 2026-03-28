//! `camel journal inspect` — offline inspection of a redb runtime journal file.

use camel_core::{JournalInspectFilter, RedbRuntimeEventJournal};

#[derive(clap::Args)]
pub struct JournalInspectArgs {
    /// Path to the `.db` journal file.
    pub path: std::path::PathBuf,

    /// Show only the last N events (default: 100).
    #[arg(long, default_value = "100")]
    pub limit: usize,

    /// Filter to a specific route_id.
    #[arg(long)]
    pub route: Option<String>,

    /// Output format: table or json.
    #[arg(long, default_value = "table")]
    pub format: OutputFormat,
}

#[derive(Clone, clap::ValueEnum)]
pub enum OutputFormat {
    Table,
    Json,
}

pub async fn run_inspect(args: JournalInspectArgs) {
    let filter = JournalInspectFilter {
        route_id: args.route.clone(),
        limit: args.limit,
    };

    let entries = match RedbRuntimeEventJournal::inspect(args.path.clone(), filter).await {
        Ok(e) => e,
        Err(err) => {
            eprintln!("error: {err}");
            std::process::exit(1);
        }
    };

    match args.format {
        OutputFormat::Table => {
            println!(
                "{:<8}  {:<26}  {:<24}  ROUTE_ID",
                "SEQ", "TIMESTAMP", "EVENT"
            );
            println!("{}", "-".repeat(80));
            if entries.is_empty() {
                println!("(no events)");
                return;
            }
            for entry in &entries {
                let ts = chrono::DateTime::from_timestamp_millis(entry.timestamp_ms)
                    .map(|dt| dt.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
                    .unwrap_or_else(|| "?".to_string());
                let (event_name, route_id) = event_parts(&entry.event);
                println!(
                    "{:>08}  {:<26}  {:<24}  {}",
                    entry.seq, ts, event_name, route_id
                );
            }
        }
        OutputFormat::Json => {
            let json = serde_json::to_string_pretty(&entries).unwrap_or_else(|e| {
                eprintln!("error: json serialize: {e}");
                std::process::exit(1);
            });
            println!("{json}");
        }
    }
}

fn event_parts(event: &camel_core::RuntimeEvent) -> (&'static str, &str) {
    match event {
        camel_core::RuntimeEvent::RouteRegistered { route_id } => ("RouteRegistered", route_id),
        camel_core::RuntimeEvent::RouteStartRequested { route_id } => {
            ("RouteStartRequested", route_id)
        }
        camel_core::RuntimeEvent::RouteStarted { route_id } => ("RouteStarted", route_id),
        camel_core::RuntimeEvent::RouteFailed { route_id, .. } => ("RouteFailed", route_id),
        camel_core::RuntimeEvent::RouteStopped { route_id } => ("RouteStopped", route_id),
        camel_core::RuntimeEvent::RouteSuspended { route_id } => ("RouteSuspended", route_id),
        camel_core::RuntimeEvent::RouteResumed { route_id } => ("RouteResumed", route_id),
        camel_core::RuntimeEvent::RouteReloaded { route_id } => ("RouteReloaded", route_id),
        camel_core::RuntimeEvent::RouteRemoved { route_id } => ("RouteRemoved", route_id),
    }
}
