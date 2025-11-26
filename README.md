# Contract Addresses Scanner

This utility script helps Base engineers ensure that the contract addresses listed in our documentation are up to date with the actual onchain deployments.

## Overview

The scanner parses a Markdown file containing tables of contract addresses, queries the relevant L1 contracts (like `SystemConfig`, `DisputeGameFactory`, etc.) via RPC, and compares the documented addresses against the actual on-chain values.

## Prerequisites

- [Rust](https://www.rust-lang.org/tools/install) (latest stable version)

## Usage

### Building

To build the project:

```bash
cargo build --release
```

### Running

You can run the scanner using `cargo run`. You need to provide the path to the file you want to check and the RPC URLs for the L1 networks.

```bash
cargo run --release -- \
  --file <PATH_TO_FILE> \
  --mainnet-rpc-url <ETHEREUM_MAINNET_RPC_URL> \
  --sepolia-rpc-url <ETHEREUM_SEPOLIA_RPC_URL>
```

### Arguments

- `-f, --file <FILE>`: Path to the file to parse (required).
- `--mainnet-rpc-url <URL>`: Ethereum Mainnet RPC URL. Can also be set via `MAINNET_RPC_URL` environment variable.
- `--sepolia-rpc-url <URL>`: Ethereum Sepolia RPC URL. Can also be set via `SEPOLIA_RPC_URL` environment variable.

### Using Make

A `Makefile` is provided for convenience. You can run the example verification with:

```bash
make run
```

_Note: The `make run` command uses hardcoded public/internal RPC URLs. You may need to override them or ensure you have access._

## Input File Format

The tool expects a Markdown file where:

- Networks are denoted by H3 headers starting with `###` (e.g., `### Ethereum Mainnet`).
- Contract addresses are listed in Markdown tables.
- The table rows should contain a contract name and an address in the format `[0x...]`.

Example:

```markdown
### Base Mainnet

| Contract Name | Address |
|Data Availability Challenge | [0x...] |
```
