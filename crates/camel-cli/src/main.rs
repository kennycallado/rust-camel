use clap::{ArgMatches, Command};

fn main() {
    let matches = Command::new("camel")
        .version(env!("CARGO_PKG_VERSION"))
        .about("Command-line interface for Apache Camel in Rust")
        .subcommand_required(false)
        .arg_required_else_help(true)
        .get_matches();

    handle_commands(&matches);
}

fn handle_commands(_matches: &ArgMatches) {
    // Future: Add run, generate, manage commands here
}
