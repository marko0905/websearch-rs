//! `websearch` — the command line, a 1:1 port of the upstream `ddgs` CLI.
//! All logic lives in [`websearch_rs::cli`].

fn main() {
    std::process::exit(websearch_rs::cli::main());
}
