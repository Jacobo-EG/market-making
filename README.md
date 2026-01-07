# Crypto Market Maker - GLFT 

> **MM-GLFT** is a market making bot which leverages the model proposed by Guéant–Lehalle–Fernandez-Tapia (GLFT) in the work [Dealing with the inventory risk: a solution to the market making problem](https://doi.org/10.1007/s11579-012-0087-0)

---

## Project Overview

This project aims to implement a high performance version of the GLFT model for market making. Currently it operates on Kraken CEX and leverages the WebSockets API to consume market data and fills orders through the HTTP API. 

This bot implements the GLFT model and considering also the work in [Optimal Market Making](https://arxiv.org/abs/1605.01862). As a result, the bid and ask price are estimated as follows:

$$
bid\_price = fair\_price - (half\_spread + skew * q)
$$
$$
ask\_price = fair\_price + (half\_spread - skew * q)
$$

where

$$
skew = \sigma  \sqrt{\frac{\gamma}{2A\Delta k}(1 + \frac{\xi\Delta}{k})^{(\frac{k}{\Delta\xi} + 1)}}
$$
$$
half\_spread = \frac{1}{\xi\Delta}\log{(1 + \frac{\xi\Delta}{k})} + \frac{\Delta}{2} \sigma  \sqrt{\frac{\gamma}{2A\Delta k}(1 + \frac{\xi\Delta}{k})^{(\frac{k}{\Delta\xi} + 1)}}
$$


---

## Development

To simplify development, we use [`just`](https://github.com/casey/just), a command runner similar to `make`.

To view all available commands, run `just` in the command line.

> If you don’t have `just` installed, install it with: `cargo install just`

---

## Contributing

Contributions are more than welcome! Please:

* Submit bug reports, ideas, or improvements via GitHub Issues
* Propose changes via pull requests
* Read [CONTRIBUTING.md](https://github.com/Jacobo-EG/market-making/CONTRIBUTING.md)

Next steps include:

* Visualisation using of the bots status trhough Grafana
* Support other CEXs/DEXs

---

## License

Licensed under either [MIT License](https://opensource.org/licenses/MIT) or [Apache License 2.0](https://www.apache.org/licenses/LICENSE-2.0) at your option.

---

## Acknowledgments

Huge thanks to [@fran0x](https://github.com/fran0x) for the helpful comments and support!