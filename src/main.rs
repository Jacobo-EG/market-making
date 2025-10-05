use std::{process, env, error::Error};
use figment::{Figment, providers::{Format, Toml}};
use market_making::Config;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {

    println!("Reading configuration file ...");

    let config_file_path = env::var("CONFIG_PATH")?;
    let fig = Figment::new().merge(Toml::file(config_file_path));

    let config: Config = fig.extract()?;

    println!("Starting Market Making using GLFT model ...");

    if let Err(e) = market_making::run(config).await {
        eprintln!("Application error: {e}");
        process::exit(1);
    }

    println!("Ending Market Making ...");

    Ok(())
}