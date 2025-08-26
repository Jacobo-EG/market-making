use std::process;


fn main() {
    println!("Starting Market Making using GLFT model ...");

    if let Err(e) = market_making::run() {
        eprintln!("Application error: {e}");
        process::exit(1);
    }

    println!("Ending Market Making ...");
}