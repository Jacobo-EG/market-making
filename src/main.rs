use krakenrs::ws::{
    BookEntry, KrakenWsAPI, KrakenWsConfig 
};


use core::f64;
use std::{
    thread,
    time::{Duration},
    collections::BTreeMap,
};

//To convert the decimal type to f64
use rust_decimal::prelude::ToPrimitive;

// To craft the API requests
use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, HeaderValue};
use base64::{engine::general_purpose, Engine as _};
use hmac::{Hmac, Mac};
use sha2::{Sha256, Sha512, Digest};
use std::time::{SystemTime, UNIX_EPOCH};
use std::collections::HashMap;

// Importing the Decimal type from the rust_decimal crate
// This type can be used for precise decimal arithmetic, especially useful in financial applications
// However, we will trade-off precision for performance by using f64 instead
// use rust_decimal::Decimal;


// Function to calculate trading intensity
fn trading_intensity(arrival_depth: &Vec<f64>, tmp: &mut Vec<f64>) -> Vec<f64> {

    let mut max_tick = 0;

    for depth in arrival_depth.iter() {
        if !depth.is_finite() {
            continue;
        }

        let tick = (depth / 0.5) as i32 - 1 ;

        if tick < 0 || tick > tmp.len() as i32{
            continue;
        }

        for i in 0..tick as usize {
            tmp[i] += 1.0;
        }
        
        if tick > max_tick {
            max_tick = tick;
        }
    }

    tmp[..max_tick as usize].to_vec()
}

// Function to calculate coefficients c1 and c2 used to calculate bid and ask quote depth
fn c1_c2(xi: f64, gamma: f64, delta: f64, A: f64, k: f64) -> (f64, f64) {
    let c1 = (1.0 + xi * delta / k).ln() / (xi * delta);
    let c2 = ((gamma / (2.0 * A * delta * k)) * (1.0 + xi * delta / k).powf(k / (xi * delta) + 1.0)).sqrt();
    (c1, c2)
}

// Function to calculate the linear regression coefficients (slope and intercept)
// We use this to callibrate lambda = A * exp(-k * delta), which is the same as log(lambda) = log(A) - k * delta
fn linear_regression(x: &[f64], y: &[f64]) -> (f64, f64) {
    let n = x.len() as f64;
    let sum_x: f64 = x.iter().sum();
    let sum_y: f64 = y.iter().sum();
    let sum_xy: f64 = x.iter().zip(y).map(|(a, b)| a * b).sum();
    let sum_x2: f64 = x.iter().map(|a| a * a).sum();

    let slope = (n * sum_xy - sum_x * sum_y) / (n * sum_x2 - sum_x * sum_x);
    let intercept = (sum_y - slope * sum_x) / n;

    (slope, intercept)
}

// Auziliary function to calculate the standard deviation ignoring NaN values
fn nanstd(slice: &[f64]) -> f64 {
    let valid: Vec<f64> = slice.iter().cloned().filter(|x| x.is_finite()).collect();
    let n = valid.len() as f64;
    if n == 0.0 {
        return f64::NAN;
    }
    let mean = valid.iter().sum::<f64>() / n;
    let var = valid.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n;
    var.sqrt()
}


// Auxiliary functions to send the HTTP requests to the Kraken API
type HmacSha512 = Hmac<Sha512>;

fn kraken_sign(api_path: &str, nonce: &str, post_data: &str, api_secret: &str) -> String {
    // 1. SHA256(nonce + POST data)
    let mut sha256 = Sha256::new();
    sha256.update(nonce.as_bytes());
    sha256.update(post_data.as_bytes());
    let hash = sha256.finalize();

    // 2. Concatenate api_path + hash
    let mut data = Vec::new();
    data.extend_from_slice(api_path.as_bytes());
    data.extend_from_slice(&hash);

    // 3. HMAC-SHA512 with base64 decoded secret
    let secret_decoded = general_purpose::STANDARD.decode(api_secret).unwrap();
    let mut mac = HmacSha512::new_from_slice(&secret_decoded).unwrap();
    mac.update(&data);
    let signature = mac.finalize().into_bytes();
    general_purpose::STANDARD.encode(signature)
}

fn kraken_add_order(client: &Client, api_key: &str, api_secret: &str, pair: &str, ordertype: &str,  volume: f64, price: f64,) -> Result<String, reqwest::Error> {
    let url = "https://api.kraken.com/0/private/AddOrder";
    let api_path = "/0/private/AddOrder";
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis().to_string();
    let mut params = HashMap::new();

    let price_ = price.to_string();
    let volume_ = volume.to_string();

    params.insert("nonce", nonce.as_str());
    params.insert("pair", pair);
    params.insert("type", ordertype);
    params.insert("ordertype", "limit");
    params.insert("price", &price_);
    params.insert("volume", &volume_);

    let post_data = format!(
        "nonce={}&pair={}&type={}&ordertype=limit&price={}&volume={}",
        nonce, pair, ordertype, price_, volume_
    );
    let api_sign = kraken_sign(api_path, &nonce, &post_data, api_secret);

    let mut headers = HeaderMap::new();
    headers.insert("API-Key", HeaderValue::from_str(api_key).unwrap());
    headers.insert("API-Sign", HeaderValue::from_str(&api_sign).unwrap());

    let res = client
        .post(url)
        .headers(headers)
        .form(&params)
        .send()?;
    let text = res.text()?;
    Ok(text)
}

fn main() {

    // Pair to subscribe to
    let pairs = vec!["XBT/USD".to_string()];

    // Create a new Kraken WebSocket API instance
    // This will connect to the Kraken WebSocket API and subscribe to the order book for the
    let ws_config = KrakenWsConfig {
        subscribe_book: pairs.clone(),
        book_depth: 100,
        private: None,
    };

    let api_ws = KrakenWsAPI::new(ws_config).expect("could not connect to websockets api");

    // GLFT Market Making Model deployed online using websockets (HTTP API for orders since WS is not supported for orders in demo mode)
    let mut out: Vec<f64> = vec![f64::NAN; 10_000_000 * 5];
    let mut arrival_depth: Vec<f64> = vec![f64::NAN; 10_000_000];
    let mut mid_price_chg: Vec<f64> = vec![f64::NAN; 10_000_000];
    let mut position: Vec<f64> = vec![0.0; 10_000_000];

    let mut tmp: Vec<f64> = vec![f64::NAN; 500];
    let ticks: Vec<f64> = (0..tmp.len()).map(|i| i as f64 + 0.5).collect();
    
    let mut t = 0;

    let mut prev_mid_price_tick = f64::NAN;
    let mut mid_price_tick = f64::NAN;

    let mut A: f64 = f64::NAN;
    let mut k: f64 = f64::NAN;
    let mut sigma: f64 = f64::NAN;
    let gamma: f64 = 0.05;
    let delta: f64 = 1.0;

    let qty: u8 = 1;
    let max_open_orders: u8 = 20;
    let tick_size: f64 = 0.1;

    loop {
        thread::sleep(Duration::from_millis(100));
        
        // Fetch the latest order book data
        let books = api_ws.get_all_books();

        for (pair, book) in books {

            // We start by recording market order's arrival depth from the mid-price.
            if !mid_price_tick.is_nan(){
                let mut depth = f64::MIN;
                for (price, _) in book.bid.iter() {
                    depth = depth.max(price.to_f64().unwrap_or(f64::NAN) / tick_size - mid_price_tick);
                }

                for (price, _) in book.ask.iter() {
                    depth = depth.max(- mid_price_tick - price.to_f64().unwrap_or(f64::NAN) / tick_size);
                }

                arrival_depth[t] = depth;
            }

            prev_mid_price_tick = mid_price_tick;
            mid_price_tick = book.bid.iter().next().map_or(f64::NAN, |(price, _)| price.to_f64().unwrap_or(f64::NAN)) +
                             book.ask.iter().next().map_or(f64::NAN, |(price, _)| price.to_f64().unwrap_or(f64::NAN)) / 2.0;
                
            mid_price_chg[t] = mid_price_tick - prev_mid_price_tick;

            // Next we calculate parameters A, k and sigma every 5 seconds in a 10 minutes window
            if t % 50 == 0 && t >= 6000 - 1 {
                tmp.fill(0.0);
                let mut lambda = trading_intensity(&arrival_depth[t + 1 - 6000..t + 1].to_vec(), &mut tmp);
                // We take the last 70 values of lambda to calculate A and k
                lambda = lambda.iter().take(70).map(|&x| x / 600.0).collect::<Vec<f64>>(); 

                let x = &ticks[..lambda.len()];
                // Apply log to lambda to create y and y will be an array
                let y = lambda.iter().map(|&l| l.ln()).collect::<Vec<f64>>(); 
                let (k_, log_a) = linear_regression(x, &y);

                A = log_a.exp();
                k = - k_;

                sigma = nanstd(&mid_price_chg[t + 1 - 6000..t + 1]) * (10.0_f64.sqrt());

                out[t * 5 + 2] = sigma;
                out[t * 5 + 3] = A;
                out[t * 5 + 4] = k;
            }
            
            // Following we calculate target bid and ask price

            let (c1, c2) = c1_c2(gamma, gamma, delta, A, k);
            
            let half_spread = c1 + delta / 2_f64 * c2 * sigma;
            let skew = c2 * sigma;

            out[t * 5 + 0] = half_spread;
            out[t * 5 + 1] = skew;

            // We use the current position to calculate the bid and ask depth
            let bid_depth = half_spread + skew * position[t];
            let ask_depth = half_spread - skew * position[t];

            let best_bid_tick = book.bid.iter().next().map_or(f64::NAN, |(price, _)| price.to_f64().unwrap_or(f64::NAN) / tick_size);
            let bid_tick = (mid_price_tick - bid_depth).round();
            let bid_price = bid_tick.min(best_bid_tick) * tick_size;

            let best_ask_tick = book.ask.iter().next().map_or(f64::NAN, |(price, _)| price.to_f64().unwrap_or(f64::NAN) / tick_size);
            let ask_tick = (mid_price_tick + ask_depth).round();
            let ask_price = ask_tick.max(best_ask_tick) * tick_size;

            // We now have everything we need to place orders
            // Here we will have a bottleneck since orders are placed thorugh HTTP API
            // which is slower but we have no other option in demo mode

            // Create connection to the HTTP API using krakenrs
            let api_key = "";
            let api_secret = "";
            let client = reqwest::blocking::Client::new();

            if position[t] < max_open_orders as f64 {
                // Place bid order if bid price is not NaN and finite
                if bid_price.is_normal() {
                    let _ = kraken_add_order(&client, &api_key, &api_secret, &pair.clone(), "buy", qty as f64, bid_price);
                    position[t] += qty as f64;
                }
                if ask_price.is_normal() {
                    let _ = kraken_add_order(&client, &api_key, &api_secret, &pair.clone(), "sell", qty as f64, ask_price);
                    position[t] -= qty as f64;
                }
            }

        }

        t+=1;

    }
    
}