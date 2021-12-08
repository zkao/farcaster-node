[![GitHub Workflow Status](https://img.shields.io/github/workflow/status/farcaster-project/farcaster-node/Build%20binaries)](https://github.com/farcaster-project/farcaster-node/actions/workflows/binaries.yml)
[![unsafe forbidden](https://img.shields.io/badge/unsafe-forbidden-success.svg)](https://github.com/rust-secure-code/safety-dance)
[![Crates.io](https://img.shields.io/crates/v/farcaster_node.svg)](https://crates.io/crates/farcaster_node)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![MSRV](https://img.shields.io/badge/MSRV-1.55.0-blue)](https://blog.rust-lang.org/2021/09/09/Rust-1.55.0.html)

# Farcaster: cross-chain atomic swaps

:warning: **THIS IS UNFINISHED, EXPERIMENTAL TECH AND YOU WILL LOSE YOUR MONEY IF YOU TRY IT ON MAINNET**

The **Farcaster Node** is _a collection of microservices for running cross-chain atomic swaps_. Currently the node is focused on Bitcoin-Monero atomic swaps, but is designed to be flexible and integrate new crypto-pairs in the future.

Microservices currently implemented:

- farcasterd (1 instance): the swap manager, it is aware of every initiated swap and interconnects all the other microservices, launches and kills other microservices, exposes an API for the swap-cli client
- swapd (1 instance per swap): control centre for an individual swap -- keeps track of the swap's state as it runs the protocol's state machine, and orchestrates the swap with peerd for communicating with swap counterparty, walletd for signing, and syncers for blockchain interactions.
- walletd (1 instance): where secret keys live, where transactions are signed, coordinates with swapd
- swap-cli: stateless terminal client (=executes a single command and terminates) that commands farcasterd, for taking or making offers, for example
- peerd (1 instance per peer connection): handles the connection to an individual peer,
- syncerd (1 instance per blockchain, i.e. one for monero and one for bitcoin): interface for getting updates of the blockchain and for broadcasting transactions

Farcaster Node is build on atomic swap primitives described in the [RFCs](https://github.com/farcaster-project/RFCs) and implemented in [Farcaster Core](https://github.com/farcaster-project/farcaster-core).

:information_source: This work is based on LNP/BP work, this project is a fork from [LNP-BP/lnp-node](https://github.com/LNP-BP/lnp-node) since [acbb4c](https://github.com/farcaster-project/farcaster-node/commit/acbb4c467695dc3d1c02b88be97e9a6e2d434435).

## Build and run

Follow the instruction for [`installing the node`](./doc/install-guide.md) on your machine by compiling sources or using containers. Containers might be your best bet, for a quick try.

Depending on the chosen installation method:
* you now have to continue to [build from sources](#build-from-sources)
* or continue to [run with docker](#run-with-docker)

They provide instructions on how to launch the swap node, codenamed `farcasterd`.

### Built from sources

If you installed the node on your machine from sources (i.e. not using Docker) you can now launch the services needed for the swap.

#### Launching Monero RPC wallet

First you need to run a `monero rpc wallet` to manage the moneros, if you have it already installed on your machine you can run

```
monero-wallet-rpc --stagenet --rpc-bind-port 38083\
    --disable-rpc-login\
    --daemon-host stagenet.melo.tools:38081\
    --trusted-daemon\
    --password "soMeDummYPasSwOrd"\
    --wallet-dir ~/.fc_monero_wallets
```

or you can use the Docker image

```
docker run --rm -p 38083:38083 ghcr.io/farcaster-project/containers/monero-wallet-rpc:latest\
    /usr/bin/monero-wallet-rpc --stagenet\
    --disable-rpc-login --wallet-dir wallets\
    --daemon-host stagenet.melo.tools:38081\
    --rpc-bind-ip 0.0.0.0 --rpc-bind-port 38083\
    --confirm-external-bind
```

#### Launching `farcasterd`

Now that you have a working Monero RPC wallet to connect you can launch the node in verbose mode (`-vv`), and follow the logs to see what's appening.

```
farcasterd -vv
```

:mag_right: You can find more details about the [configuration](#configuration) below.

#### Connect a client

Once `farcasterd` is up & running you can issue commands to control its actions with a client. For the time being, only one client is provided within this repo: `swap-cli`.

If you launched `farcasterd` with the default paramters (the `--data-dir` argument or `-d`), `swap-cli` will be able to connect to `farcasterd` without further configuration. You can get informations about the node with (this require `swap-cli` to be installed on your host):

```
swap-cli info
```

Run `help` command for more details about available commands.

Commands you should know: `swap-cli info` gives a genaral overview of the node, `swap-cli ls` lists the ongoing swaps and `swap-cli progress <swap_id>` give the state of a given swap.

With those commands and farcasterd logs you should be able to follow your swaps.

Checkout the documentaion on [how to use the node](#usage) for more details.

### Run with docker

If you did use Docker you are already all setup, the node and the wallet are running and you can interact with `farcasterd` container (when in the same folder as the `docker-compose.yml`) using the cli

```
docker-compose exec farcasterd swap-cli info
```

Commands you should know: `swap-cli info` gives a genaral overview of the node, `swap-cli ls` lists the ongoing swaps and `swap-cli progress <swap_id>` give the state of a given swap.

With those commands and farcasterd logs (attach to the log with `docker-compose logs -f --no-log-prefix farcasterd`) you should be able to follow your swaps.

Checkout the documentaion on [how to use the node](#usage) for more details.

### Configuration

`farcasterd` can be configured through a toml file located by default at `~/.farcaster/farcasterd.toml` (Linux and BSD, see [here](./src/opts.rs) for more platforms specific), if no file is found `farcasterd` is launched with some default values. You can see an example [here](./farcasterd.toml).

**Syncers**

Configures daemon's URLs where to connect for the three possible networks: _mainnet_, _testnet_, _local_:

```toml
[syncers.{network}]
electrum_server = ""
monero_daemon = ""
monero_rpc_wallet = ""
```

:mag_right: The default config for _local_ network is set to null.

#### :bulb: Use public infrastructure

To help quickly test and avoid running the entire infrastructure on your machine you can make use of public nodes. Following is a non-exhaustive list of public nodes.

Only blockchain daemons and electrum servers are listed, you should always run your own `monero rpc wallet`.

**Mainnet**

| daemon            | value                                                |
| ----------------- | ---------------------------------------------------- |
| `electrum server` | `ssl://blockstream.info:700` **(default)**           |
| `monero daemon`   | `http://node.melo.tools:18081`                       |
| `monero daemon`   | `http://node.monerooutreach.org:18081` **(default)** |

**Testnet/Stagenet**

| daemon            | value                                            |
| ----------------- | ------------------------------------------------ |
| `electrum server` | `ssl://blockstream.info:993` **(default)**       |
| `monero daemon`   | `http://stagenet.melo.tools:38081` **(default)** |

## Usage

:rotating_light: **The following section focus on how to use the Farcaster Node to propose and run atomic swaps. Keep in mind that this software remains experimental and should not be used on mainnet or with any valuable assets.**

When `farcasterd` is up & running and `swap-cli` is configured to connect and control it, you can make offers and/or take offers. An offer encapsulate informations about a trade of Bitcoin and Monero. One will make :hammer: an offer, e.g. a market maker, and one will try to take :moneybag: the offer. Below are the commands to use to either `make` an offer or `take` one.

### :hammer: Make an offer

When making an offer, the maker starts listening for other peers to connect and take that offer -- and hopefully succesfully execute a swap. 

A `peerd` instance is spawned by the maker and binds to the specified `address:port`. The takers `farcasterd` then launches its own `peerd` that connects to the makers `peerd`, the communication is then established between two nodes, and they can swap.

:mag_right: This requires for the time being some notions about the network topology the maker node is running in, this requirement will be lifted off later when integrating Tor by default.

If you are the maker, to make an offer and spawn a listener awaiting for takers to take that offer, run the following command:

```
swap-cli make --btc-addr tb1q935eq5fl2a3ajpqp0e3d7z36g7vctcgv05f5lf\
    --xmr-addr 54EYTy2HYFcAXwAbFQ3HmAis8JLNmxRdTC9DwQL7sGJd4CAUYimPxuQHYkMNg1EELNP85YqFwqraLd4ovz6UeeekFLoCKiu\
    --btc-amount "0.0000135 BTC" --xmr-amount "0.001 XMR"\
    --network testnet --arb-blockchain bitcoin --acc-blockchain monero\
    --maker-role Bob --cancel-timelock 4 --punish-timelock 5 --fee-strategy "1 satoshi/vByte"\
    --public-ip-addr 1.2.3.4 --bind-ip-addr 0.0.0.0 --port 9735 --overlay tcp
```

Network and assets by default are Bitcoin and Monero on testnet. The first arguments `--btc-addr` and `--xmr-addr` are the Bitcoin and Monero addresses used to get the bitcoins and moneros as a refund or when the swap completes depending on the role. They are followed by the amounts exchanged.

The role for the maker is specified in the offer with `--maker-role`. `Alice` sells moneros for bitcoins, `Bob` sells bitcoins for moneros. Timelock parameters are set to **4** and **5** for cancel and punish and the transaction fee that must be applied is **1 satoshi per vByte**.

Here the maker will send bitcoins and will receive moneros in his `54EYTy2HYFcAXwAbFQ3HmAis8JLNmxRdTC9DwQL7sGJd4CAUYimPxuQHYkMNg1EELNP85YqFwqraLd4ovz6UeeekFLoCKiu` address if the swap is successful.

`--public-ip-addr` (default to `127.0.0.1`) and `--port` (default to `9735`) are used in the public offer for the taker to connect. `--bind-ip-addr` allow to bind the listening peerd to `0.0.0.0`, `tcp` is used as overlay between peers.

:mag_right: To be able for a taker to connect and take the offer the `public-ip-addr:port` must be accessible and answered by the `peerd` binded to `bind-id-address:port`.

**The public offer result**

The command will ouput an encoded **public offer** that must be shared to anyone susceptible to take it and your `farcasterd` will register this public offer in its list, waiting for someone to connect and take it.

Follow your `farcasterd` log (**with a log level set at `-vv`**) and fund the swap with the bitcoins or moneros when it asks so, at the end you should receive the counterparty assets.

### :moneybag: Take the offer

Taking a public offer is a much simpler process, all you need is a running node (doesn't require to know your network topology), an encoded public offer, a Bitcoin address and a Monero address to receive assets, again as a refund or as a payment depending on your swap role and if the swap completes.

```
swap-cli take --btc-addr tb1qmcku4ht3tq53tvdl5hj03rajpdkdatd4w4mswx\
    --xmr-addr 54EYTy2HYFcAXwAbFQ3HmAis8JLNmxRdTC9DwQL7sGJd4CAUYimPxuQHYkMNg1EELNP85YqFwqraLd4ovz6UeeekFLoCKiu\
    --offer {offer}
```

The cli will ask you to validate the offer informations (amounts, assets, etc.), you can use the flag of interest `--without-validation` or `-w` for externally validated automated setups.

Then follow your `farcasterd` log (**with a log level set at `-vv`**) and fund the swap with the bitcoins or moneros when it asks so, at the end you should receive the counterparty assets.

### Run a swap locally

If you want to test a swap with yourself locally you can follow the instructions [here](./doc/local-swap.md).

## Releases and Changelog

See [CHANGELOG.md](CHANGELOG.md) and [RELEASING.md](RELEASING.md).

## About

This work is part of the Farcaster cross-chain atomic swap project, see [Farcaster Project](https://github.com/farcaster-project).

## Licensing

The code in this project is licensed under the [MIT License](LICENSE).

## Ways of communication

IRC channels on Libera.chat \#monero-swap, Bitcoin-Monero cross-chain atomic swaps research and development.
