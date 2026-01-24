use figment::{
    Figment,
    providers::{Format, Toml},
};
use market_making::Config;
use std::{env, error::Error, process};
use clap::{command, Arg};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {

    console_subscriber::init();

    let match_result = command!()
    .about("Market-making bot for the Kraken crypto futures exchange")
    .arg(
        Arg::new("config_file_path")
        .short('c')
        .long("config-file")
        .aliases(["config"])
        .required(true)
        .help("Path to file to configure the bot, including pairs to trade, model's hyperparameters, ...")
    )
    .arg(
        Arg::new("api_key")
        .short('k')
        .long("api-key")
        .aliases(["key","apikey"])
        .required(true)
        .help("Api key of the bot's account")
    )
    .arg(
        Arg::new("api_secret")
        .short('s')
        .long("api-secret")
        .aliases(["secret","apisecret"])
        .required(true)
        .help("Api secret for the bot's account")
    ).get_matches();

    println!("Reading configuration file ...");

    let config_file_path = match_result.get_one::<String>("config_file_path").unwrap();
    let fig = Figment::new().merge(Toml::file(config_file_path));

    let config: Config = fig.extract()?;

    let api_key = match_result.get_one::<String>("api_key").unwrap();
    let api_secret = match_result.get_one::<String>("api_secret").unwrap();

    println!("Starting Market Making using GLFT model ...");

    if let Err(e) = market_making::run(config, api_key, api_secret).await {
        eprintln!("Application error: {e}");
        process::exit(1);
    }

    println!("Ending Market Making ...");

    Ok(())
}
