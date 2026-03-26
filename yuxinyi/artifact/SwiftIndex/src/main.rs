use clap::Parser;

fn main() {
    let cli = repo_index::cli::Cli::parse();
    if let Err(error) = repo_index::cli::run(cli) {
        eprintln!("Error: {error:#}");
        std::process::exit(1);
    }
}
