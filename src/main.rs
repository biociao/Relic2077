use clap::Parser;

fn main() {
    if let Err(error) = relic2077::cli::Cli::parse().run() {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}
