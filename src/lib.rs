use std::{
    collections::{BTreeMap, HashMap},
    env,
    error::Error,
    str::FromStr,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
type HmacSha512 = Hmac<Sha512>;
use core::f64;

use futures_util::{SinkExt, StreamExt};
use serde::{self, Deserialize};
use serde_json::Value;

use tokio::sync::mpsc;
use tokio::time::sleep;

//To convert the decimal type to f64
use rust_decimal::{Decimal, prelude::ToPrimitive};

// To craft the API requests
use base64::{Engine as _, engine::general_purpose};
use hmac::{Hmac, Mac};
use reqwest::{
    Client,
    header::{HeaderMap, HeaderValue},
};
use sha2::{Digest, Sha256, Sha512};

// Used to retrieve date in YYYYMMDD format to generate the crl_ord_id
use chrono::Utc;

use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};

// Importing the Decimal type from the rust_decimal crate
// This type can be used for precise decimal arithmetic, especially useful in financial applications
// However, we will trade-off precision for performance by using f64 instead
// use rust_decimal::Decimal;

#[derive(Deserialize)]
pub struct Config {
    pub pairs: Vec<String>,
    pub buffer_size: usize,
    pub a: f64,
    pub k: f64,
    pub sigma: f64,
    pub gamma: f64,
    pub delta: f64,
    pub qty: u8,
    pub max_open_orders: u8,
    pub tick_size: f64,
    pub time_to_sleep: u64,
    pub book_depth: usize,
}

// Function to calculate trading intensity
pub fn trading_intensity<'a>(arrival_depth: &[f64], tmp: &'a mut [f64]) -> Vec<f64> {
    //<'a>(arrival_depth: &[f64], tmp: &'a mut [f64]) -> &'a [f64] {
    let mut max_tick = 0;

    for depth in arrival_depth.iter() {
        if !depth.is_finite() {
            continue;
        }

        let tick = (depth / 0.5) as i32 - 1;

        if tick < 0 || tick > tmp.len() as i32 {
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
pub fn c1_c2(xi: f64, gamma: f64, delta: f64, a: f64, k: f64) -> (f64, f64) {
    let c1 = (1.0 + xi * delta / k).ln() / (xi * delta);
    let c2 = ((gamma / (2.0 * a * delta * k))
        * (1.0 + xi * delta / k).powf(k / (xi * delta) + 1.0))
    .sqrt();
    (c1, c2)
}

// Function to calculate the linear regression coefficients (slope and intercept)
// We use this to callibrate lambda = A * exp(-k * delta), which is the same as log(lambda) = log(A) - k * delta
pub fn linear_regression(x: &[f64], y: &[f64]) -> (f64, f64) {
    let n = x.len() as f64;
    let sum_x: f64 = x.iter().sum();
    let sum_y: f64 = y.iter().sum();
    let sum_xy: f64 = x.iter().zip(y).map(|(a, b)| a * b).sum();
    let sum_x2: f64 = x.iter().map(|a| a * a).sum();

    let slope = (n * sum_xy - sum_x * sum_y) / (n * sum_x2 - sum_x * sum_x);
    let intercept = (sum_y - slope * sum_x) / n;

    (slope, intercept)
}

// Auxiliary function to calculate the standard deviation ignoring NaN values
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
fn kraken_sign(api_path: &str, nonce: &str, post_data: &str, api_secret: &str) -> String {
    let mut sha256 = Sha256::new();
    sha256.update(nonce.as_bytes());
    sha256.update(post_data.as_bytes());
    let hash = sha256.finalize();

    let mut data = Vec::new();
    data.extend_from_slice(api_path.as_bytes());
    data.extend_from_slice(&hash);

    let secret_decoded = general_purpose::STANDARD.decode(api_secret).unwrap();
    let mut mac = HmacSha512::new_from_slice(&secret_decoded).unwrap();
    mac.update(&data);
    let signature = mac.finalize().into_bytes();
    general_purpose::STANDARD.encode(signature)
}

#[derive(Clone, Debug)]
struct BookLevels {
    bids: Vec<(Decimal, Decimal)>,
    asks: Vec<(Decimal, Decimal)>,
}

#[derive(Clone, Debug)]
struct BookUpdate {
    pair: String,
    levels: BookLevels,
}

struct OrderBook {
    depth: usize,
    bids: BTreeMap<Decimal, Decimal>,
    asks: BTreeMap<Decimal, Decimal>,
}

impl OrderBook {
    fn new(depth: usize) -> Self {
        Self {
            depth,
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
        }
    }

    fn apply_snapshot(&mut self, bids: &[(Decimal, Decimal)], asks: &[(Decimal, Decimal)]) {
        self.bids.clear();
        self.asks.clear();

        for (price, volume) in bids {
            if !volume.is_zero() {
                self.bids.insert(price.clone(), volume.clone());
            }
        }
        for (price, volume) in asks {
            if !volume.is_zero() {
                self.asks.insert(price.clone(), volume.clone());
            }
        }

        self.trim_to_depth();
    }

    fn apply_updates(
        &mut self,
        bid_updates: &[(Decimal, Decimal)],
        ask_updates: &[(Decimal, Decimal)],
    ) {
        for (price, volume) in bid_updates {
            if volume.is_zero() {
                self.bids.remove(price);
            } else {
                self.bids.insert(price.clone(), volume.clone());
            }
        }

        for (price, volume) in ask_updates {
            if volume.is_zero() {
                self.asks.remove(price);
            } else {
                self.asks.insert(price.clone(), volume.clone());
            }
        }

        self.trim_to_depth();
    }

    fn trim_to_depth(&mut self) {
        while self.bids.len() > self.depth {
            if let Some(key) = self.bids.iter().next().map(|(price, _)| price.clone()) {
                self.bids.remove(&key);
            } else {
                break;
            }
        }

        while self.asks.len() > self.depth {
            if let Some(key) = self.asks.iter().next_back().map(|(price, _)| price.clone()) {
                self.asks.remove(&key);
            } else {
                break;
            }
        }
    }

    fn to_levels(&self) -> BookLevels {
        let bids = self
            .bids
            .iter()
            .rev()
            .take(self.depth)
            .map(|(price, volume)| (price.clone(), volume.clone()))
            .collect();

        let asks = self
            .asks
            .iter()
            .take(self.depth)
            .map(|(price, volume)| (price.clone(), volume.clone()))
            .collect();

        BookLevels { bids, asks }
    }
}

fn parse_decimal(value: &Value) -> Option<Decimal> {
    match value {
        Value::String(s) => Decimal::from_str(s).ok(),
        Value::Number(num) => Decimal::from_str(&num.to_string()).ok(),
        _ => None,
    }
}

fn parse_levels(value: &Value) -> Vec<(Decimal, Decimal)> {
    value
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| {
                    let arr = entry.as_array()?;
                    if arr.len() < 2 {
                        return None;
                    }
                    let price = parse_decimal(&arr[0])?;
                    let volume = parse_decimal(&arr[1])?;
                    Some((price, volume))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn format_client_order_id(seq: u64) -> String {
    format!(
        "bot_{}_{}",
        Utc::now().format("%Y%m%d").to_string(),
        format!("{:05}", seq)
    )
}

async fn stream_order_books(
    pairs: Vec<String>,
    depth: usize,
    tx: mpsc::Sender<BookUpdate>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let subscribe_message = serde_json::json!({
        "event": "subscribe",
        "pair": pairs,
        "subscription": {"name": "book", "depth": depth}
    })
    .to_string();

    loop {
        match connect_async("wss://ws.kraken.com").await {
            Ok((ws_stream, _)) => {
                let (mut write, mut read) = ws_stream.split();
                write
                    .send(Message::Text(subscribe_message.clone().into()))
                    .await?;

                let mut books: HashMap<String, OrderBook> = HashMap::new();

                while let Some(message) = read.next().await {
                    match message {
                        Ok(Message::Text(payload)) => {
                            let value: Value = match serde_json::from_str(&payload) {
                                Ok(v) => v,
                                Err(_) => continue,
                            };

                            if let Some(obj) = value.as_object() {
                                if let Some(event) = obj.get("event").and_then(|v| v.as_str()) {
                                    match event {
                                        "heartbeat" => continue,
                                        "systemStatus" => continue,
                                        "subscriptionStatus" => {
                                            if obj.get("status").and_then(|v| v.as_str())
                                                != Some("subscribed")
                                            {
                                                tracing::warn!(?obj, "Subscription not confirmed");
                                            }
                                            continue;
                                        }
                                        _ => continue,
                                    }
                                }
                            }

                            let array = match value.as_array() {
                                Some(arr) => arr,
                                None => continue,
                            };

                            if array.len() < 4 {
                                continue;
                            }

                            let pair = match array.last().and_then(|v| v.as_str()) {
                                Some(p) => p.to_string(),
                                None => continue,
                            };

                            let order_book = books
                                .entry(pair.clone())
                                .or_insert_with(|| OrderBook::new(depth));

                            let mut snapshot_bids = Vec::new();
                            let mut snapshot_asks = Vec::new();
                            let mut update_bids = Vec::new();
                            let mut update_asks = Vec::new();
                            let mut is_snapshot = false;

                            let data_entries = &array[1..array.len() - 2];
                            for entry in data_entries {
                                if let Some(obj) = entry.as_object() {
                                    if let Some(asks) = obj.get("as") {
                                        snapshot_asks = parse_levels(asks);
                                        is_snapshot = true;
                                    }
                                    if let Some(bids) = obj.get("bs") {
                                        snapshot_bids = parse_levels(bids);
                                        is_snapshot = true;
                                    }
                                    if let Some(asks) = obj.get("a") {
                                        update_asks.extend(parse_levels(asks));
                                    }
                                    if let Some(bids) = obj.get("b") {
                                        update_bids.extend(parse_levels(bids));
                                    }
                                }
                            }

                            if is_snapshot {
                                order_book.apply_snapshot(&snapshot_bids, &snapshot_asks);
                            }

                            if !update_bids.is_empty() || !update_asks.is_empty() {
                                order_book.apply_updates(&update_bids, &update_asks);
                            }

                            if is_snapshot || !update_bids.is_empty() || !update_asks.is_empty() {
                                let levels = order_book.to_levels();
                                if tx
                                    .send(BookUpdate {
                                        pair: pair.clone(),
                                        levels,
                                    })
                                    .await
                                    .is_err()
                                {
                                    return Ok(());
                                }
                            }
                        }
                        Ok(Message::Ping(payload)) => {
                            write.send(Message::Pong(payload)).await?;
                        }
                        Ok(Message::Pong(_)) => {}
                        Ok(Message::Close(_)) => break,
                        Err(e) => {
                            tracing::warn!(error = %e, "WebSocket receive error");
                            break;
                        }
                        _ => {}
                    }
                }
            }
            Err(e) => {
                tracing::error!(error = %e, "Failed to connect to Kraken WebSocket");
            }
        }

        sleep(Duration::from_secs(2)).await;
    }
}

async fn kraken_add_order(
    client: &Client,
    api_key: &str,
    api_secret: &str,
    pair: &str,
    ordertype: &str,
    volume: f64,
    price: f64,
    crl_ord_id: &str,
) -> Result<String, reqwest::Error> {
    let url = "https://demo-futures.kraken.com/0/private/AddOrder";
    let api_path = "/0/private/AddOrder";
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis()
        .to_string();
    let mut params = HashMap::new();

    let price_ = price.to_string();
    let volume_ = volume.to_string();

    params.insert("nonce", nonce.as_str());
    params.insert("pair", pair);
    params.insert("type", ordertype);
    params.insert("ordertype", "limit");
    params.insert("price", &price_);
    params.insert("volume", &volume_);
    params.insert("cl_ord_id", crl_ord_id);

    let post_data = format!(
        "nonce={}&pair={}&type={}&ordertype=limit&price={}&volume={}&cl_ord_id={}",
        nonce, pair, ordertype, price_, volume_, crl_ord_id
    );

    let api_sign = kraken_sign(api_path, &nonce, &post_data, api_secret);

    let mut headers = HeaderMap::new();
    headers.insert("API-Key", HeaderValue::from_str(api_key).unwrap());
    headers.insert("API-Sign", HeaderValue::from_str(&api_sign).unwrap());

    let res = client
        .post(url)
        .headers(headers)
        .form(&params)
        .send()
        .await?;

    let text = res.text().await?;

    Ok(text)
}

pub async fn run(config: Config) -> Result<(), Box<dyn Error>> {
    let Config {
        pairs,
        buffer_size,
        a: initial_a,
        k: initial_k,
        sigma: initial_sigma,
        gamma,
        delta,
        qty,
        max_open_orders,
        tick_size,
        time_to_sleep,
        book_depth,
    } = config;

    let api_key = env::var("KRAKEN_API_KEY")?;
    let api_secret = env::var("KRAKEN_API_SECRET")?;

    let (book_tx, mut book_rx) = mpsc::channel(256);
    tokio::spawn({
        let subscribe_pairs = pairs.clone();
        async move {
            if let Err(e) = stream_order_books(subscribe_pairs, book_depth, book_tx).await {
                tracing::error!(error = %e, "Book stream task ended");
            }
        }
    });

    let mut out = vec![f64::NAN; buffer_size * 5];
    let mut arrival_depth = vec![f64::NAN; buffer_size];
    let mut mid_price_chg = vec![f64::NAN; buffer_size];
    let mut position = vec![0.0; buffer_size];

    let mut tmp = vec![f64::NAN; 3_000_000];
    let ticks: Vec<f64> = (0..tmp.len()).map(|i| i as f64 + 0.5).collect();

    let mut t = 0;
    let mut num_orders: u32 = 0;

    let mut prev_mid_price_tick: f64;
    let mut mid_price_tick = f64::NAN;

    let mut a = initial_a;
    let mut k = initial_k;
    let mut sigma = initial_sigma;

    let mut books_state: HashMap<String, BookLevels> = HashMap::new();

    loop {
        sleep(Duration::from_millis(time_to_sleep)).await;

        loop {
            match book_rx.try_recv() {
                Ok(update) => {
                    books_state.insert(update.pair, update.levels);
                }
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                    return Err("Order book stream disconnected".into());
                }
            }
        }

        let idx = t % buffer_size;

        for pair in &pairs {
            let Some(book) = books_state.get(pair) else {
                continue;
            };

            if !mid_price_tick.is_nan() {
                let mut depth = f64::MIN;
                for (price, _) in &book.bids {
                    depth =
                        depth.max(price.to_f64().unwrap_or(f64::NAN) / tick_size - mid_price_tick);
                }

                for (price, _) in &book.asks {
                    depth =
                        depth.max(mid_price_tick - price.to_f64().unwrap_or(f64::NAN) / tick_size);
                }

                arrival_depth[idx] = depth;
            }

            prev_mid_price_tick = mid_price_tick;
            mid_price_tick = (book
                .bids
                .first()
                .map_or(f64::NAN, |(price, _)| price.to_f64().unwrap_or(f64::NAN))
                + book
                    .asks
                    .first()
                    .map_or(f64::NAN, |(price, _)| price.to_f64().unwrap_or(f64::NAN)))
                / 2.0;

            mid_price_chg[idx] = mid_price_tick - prev_mid_price_tick;

            if t % 50 == 0 && t >= buffer_size - 1 {
                tmp.fill(0.0);

                let mut lambda = trading_intensity(&arrival_depth, &mut tmp);
                lambda = lambda
                    .iter()
                    .take(70)
                    .map(|x| x / 600.0)
                    .collect::<Vec<f64>>();

                let x = &ticks[..lambda.len()];
                let y = lambda.iter().map(|&l| l.ln()).collect::<Vec<f64>>();
                let (k_, log_a) = linear_regression(x, &y);

                a = log_a.exp();
                k = -k_;

                sigma = nanstd(&mid_price_chg) * (10.0_f64.sqrt());

                out[idx * 5 + 2] = sigma;
                out[idx * 5 + 3] = a;
                out[idx * 5 + 4] = k;
            }

            let (c1, c2) = c1_c2(gamma, gamma, delta, a, k);

            let half_spread = c1 + delta / 2_f64 * c2 * sigma;
            let skew = c2 * sigma;

            out[idx * 5 + 0] = half_spread;
            out[idx * 5 + 1] = skew;

            let bid_depth = half_spread + skew * position[idx];
            let ask_depth = half_spread - skew * position[idx];

            let best_bid_tick = book.bids.first().map_or(f64::NAN, |(price, _)| {
                price.to_f64().unwrap_or(f64::NAN) / tick_size
            });
            let bid_tick = (mid_price_tick - bid_depth).round();
            let mut bid_price = f64::NAN;
            if bid_tick.is_normal() && best_bid_tick.is_normal() {
                bid_price = bid_tick.min(best_bid_tick) * tick_size;
            }

            let best_ask_tick = book.asks.first().map_or(f64::NAN, |(price, _)| {
                price.to_f64().unwrap_or(f64::NAN) / tick_size
            });
            let ask_tick = (mid_price_tick + ask_depth).round();
            let mut ask_price = f64::NAN;
            if ask_tick.is_normal() && best_ask_tick.is_normal() {
                ask_price = ask_tick.max(best_ask_tick) * tick_size;
            }

            if position[idx] > 0.0 && ask_price.is_normal() {
                let client = reqwest::Client::new();
                let crl_ord_id = format_client_order_id(num_orders as u64);
                kraken_add_order(
                    &client,
                    &api_key,
                    &api_secret,
                    pair,
                    "sell",
                    qty as f64,
                    ask_price,
                    &crl_ord_id,
                )
                .await?;

                position[idx] -= qty as f64;
                num_orders += 1;
            }

            if position[idx] < max_open_orders as f64 && bid_price.is_normal() {
                let client = reqwest::Client::new();
                let crl_ord_id = format_client_order_id(num_orders as u64);
                kraken_add_order(
                    &client,
                    &api_key,
                    &api_secret,
                    pair,
                    "buy",
                    qty as f64,
                    bid_price,
                    &crl_ord_id,
                )
                .await?;

                position[idx] += qty as f64;
                num_orders += 1;
            }
        }

        t += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nanstd_constant_values() {
        let x = [1.0, 1.0, 1.0, 1.0];

        assert_eq!(nanstd(&x), 0.0);
    }

    #[test]
    fn nanstd_simple_case() {
        let x = [1.0, 2.0, 3.0, 4.0, 5.0];
        let result = nanstd(&x);

        assert!((result - 1.4142135623730951).abs() < 1e-10);
    }

    #[test]
    fn nanstd_with_nan_values() {
        let x = [1.0, f64::NAN, 2.0, f64::NAN, 3.0, 4.0, 5.0];
        let expected = [1.0, 2.0, 3.0, 4.0, 5.0];
        let result = nanstd(&x);
        let expected_result = nanstd(&expected);

        assert!((result - expected_result).abs() < 1e-10);
    }

    #[test]
    fn nanstd_with_infinity() {
        let x = [1.0, 2.0, f64::INFINITY, 3.0, f64::NEG_INFINITY, 4.0];
        let expected = [1.0, 2.0, 3.0, 4.0];
        let result = nanstd(&x);
        let expected_result = nanstd(&expected);

        assert!((result - expected_result).abs() < 1e-10);
    }

    #[test]
    fn nanstd_all_nan() {
        let x = [f64::NAN, f64::NAN, f64::NAN];
        let result = nanstd(&x);

        assert!(result.is_nan());
    }

    #[test]
    fn nanstd_empty_after_filtering() {
        let x = [f64::INFINITY, f64::NEG_INFINITY, f64::NAN];
        let result = nanstd(&x);

        assert!(result.is_nan());
    }

    #[test]
    fn nanstd_single_valid_value() {
        let x = [f64::NAN, 5.0, f64::NAN];
        let result = nanstd(&x);

        assert_eq!(result, 0.0);
    }

    #[test]
    fn nanstd_negative_values() {
        let x = [-2.0, -1.0, 0.0, 1.0, 2.0];
        let result = nanstd(&x);

        assert!((result - 1.4142135623730951).abs() < 1e-10);
    }

    #[test]
    fn linear_regression_perfect_line() {
        let x = [1.0, 2.0, 3.0, 4.0, 5.0];
        let y = [5.0, 7.0, 9.0, 11.0, 13.0];
        let (slope, intercept) = linear_regression(&x, &y);

        assert!((slope - 2.0).abs() < 1e-10);
        assert!((intercept - 3.0).abs() < 1e-10);
    }

    #[test]
    fn linear_regression_negative_slope() {
        let x = [0.0, 2.0, 4.0, 6.0, 8.0];
        let y = [10.0, 9.0, 8.0, 7.0, 6.0];
        let (slope, intercept) = linear_regression(&x, &y);

        assert!((slope - (-0.5)).abs() < 1e-10);
        assert!((intercept - 10.0).abs() < 1e-10);
    }

    #[test]
    fn linear_regression_horizontal_line() {
        let x = [1.0, 2.0, 3.0, 4.0];
        let y = [5.0, 5.0, 5.0, 5.0];
        let (slope, intercept) = linear_regression(&x, &y);

        assert!((slope - 0.0).abs() < 1e-10);
        assert!((intercept - 5.0).abs() < 1e-10);
    }

    #[test]
    fn linear_regression_two_points() {
        let x = [1.0, 2.0];
        let y = [3.0, 5.0];
        let (slope, intercept) = linear_regression(&x, &y);

        assert!((slope - 2.0).abs() < 1e-10);
        assert!((intercept - 1.0).abs() < 1e-10);
    }

    #[test]
    fn linear_regression_origin_through() {
        let x = [0.0, 1.0, 2.0, 3.0, 4.0];
        let y = [0.0, 2.0, 4.0, 6.0, 8.0];
        let (slope, intercept) = linear_regression(&x, &y);

        assert!((slope - 2.0).abs() < 1e-10);
        assert!((intercept - 0.0).abs() < 1e-10);
    }

    #[test]
    fn linear_regression_large_values() {
        let x = [100.0, 200.0, 300.0, 400.0];
        let y = [1000.0, 2000.0, 3000.0, 4000.0];
        let (slope, intercept) = linear_regression(&x, &y);

        assert!((slope - 10.0).abs() < 1e-8);
        assert!((intercept - 0.0).abs() < 1e-8);
    }

    #[test]
    fn nanstd_0() {
        let x = [1.0, 1.0, 1.0, 1.0];

        assert_eq!(nanstd(&x), 0_f64)
    }
}
