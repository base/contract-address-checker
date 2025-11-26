.PHONY: run
run:
	cargo run --release -- --file examples/addresses.mdx \
	--mainnet-rpc-url https://eth-mainnet.public.blastapi.io \
	--sepolia-rpc-url https://ethereum-full-sepolia-k8s-dev.cbhq.net

.PHONY: test
test:
	cargo test
