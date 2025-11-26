use alloy::primitives::Address;
use alloy::providers::ProviderBuilder;
use alloy::sol_types::SolCall;
use anyhow::{Context, Result, anyhow};
use clap::Parser;
use regex::Regex;
use std::fs;
use std::path::PathBuf;
use std::str::FromStr;

mod abi;
use abi::{
    DisputeGameFactory, FaultDisputeGame, MIPS, Multicall3, PermissionedDisputeGame, SystemConfig,
};

mod constants;
use constants::*;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    /// Path to the file to parse
    #[arg(short, long, value_name = "FILE")]
    file: PathBuf,

    /// Mainnet RPC URL
    #[arg(long, value_name = "URL", env = MAINNET_RPC_URL_ENV)]
    mainnet_rpc_url: Option<String>,

    /// Sepolia RPC URL
    #[arg(long, value_name = "URL", env = SEPOLIA_RPC_URL_ENV)]
    sepolia_rpc_url: Option<String>,
}

#[derive(Debug)]
struct Contract {
    name: String,
    address: String,
}

#[derive(Debug)]
struct Network {
    name: String,
    contracts: Vec<Contract>,
}

#[derive(Debug)]
struct CheckResult {
    name: String,
    network: String,
    expected: Option<Address>,
    actual: Option<Address>,
    success: bool,
    error: Option<String>,
}

type Decoder = Box<dyn Fn(&[u8]) -> Result<Address> + Send + Sync>;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let content = fs::read_to_string(&cli.file)
        .with_context(|| format!("Failed to read input file: {:?}", cli.file))?;

    let networks = parse_networks(&content)?;

    // Verification Logic
    println!("\n---------------------------------------------------------------------------");
    println!("Verifying addresses...");
    println!("---------------------------------------------------------------------------");

    let mainnet_task = verify_network(
        &networks,
        ETHEREUM_MAINNET,
        BASE_MAINNET,
        cli.mainnet_rpc_url,
    );

    let sepolia_task = verify_network(
        &networks,
        ETHEREUM_SEPOLIA,
        BASE_SEPOLIA,
        cli.sepolia_rpc_url,
    );

    let (mainnet_res, sepolia_res) = tokio::join!(mainnet_task, sepolia_task);

    let mut exit_code = 0;

    for (res, network_name) in [
        (mainnet_res, ETHEREUM_MAINNET),
        (sepolia_res, ETHEREUM_SEPOLIA),
    ] {
        match res {
            Ok(results) => {
                if results.is_empty() {
                    println!(
                        "Skipped verification for {} (No RPC URL or addresses found)",
                        network_name
                    );
                    continue;
                }

                let mut network_passed = true;
                for check in results {
                    if !check.success {
                        network_passed = false;
                        exit_code = 1;
                        print_failure(&check);
                    }
                }

                if network_passed {
                    println!("✅ All addresses match for {}", network_name);
                }
            }
            Err(e) => {
                eprintln!("❌ Error verifying {}: {:#}", network_name, e);
                exit_code = 1;
            }
        }
    }

    if exit_code == 0 {
        println!("\n✅ All checks passed successfully.");
    } else {
        eprintln!("\n❌ Verification failed for one or more networks.");
    }

    std::process::exit(exit_code);
}

fn print_failure(check: &CheckResult) {
    if let Some(error) = &check.error {
        println!("❌ ERROR for {}: {}", check.name, error);
        return;
    }

    let expected = check
        .expected
        .map(|a| a.to_string())
        .unwrap_or_else(|| "Unknown".to_string());
    let actual = check
        .actual
        .map(|a| a.to_string())
        .unwrap_or_else(|| "Unknown".to_string());

    println!(
        "❌ MISMATCH for {} ({}): \n\tFile: {}\n\tChain: {}",
        check.name, check.network, expected, actual
    );
}

fn parse_networks(content: &str) -> Result<Vec<Network>> {
    // Regex to capture network headers (lines starting with ###)
    let network_re = Regex::new(r"^###\s+(?P<network>.+)")?;

    // Regex to capture contract name and address
    // Matches lines starting with | (optional), then name column, then address column containing [address]
    let contract_re =
        Regex::new(r"\|\s*(?P<name>[^|]+?)\s*\|\s*\[(?P<address>0x[a-fA-F0-9]{40})\]")?;

    let mut networks: Vec<Network> = Vec::new();
    let mut current_network_name: Option<String> = None;

    for (line_num, line) in content.lines().enumerate() {
        // Check for network header
        if let Some(caps) = network_re.captures(line) {
            let network_name = caps
                .name("network")
                .map(|m| m.as_str().trim().to_string())
                .ok_or_else(|| {
                    anyhow!("Failed to capture network name on line {}", line_num + 1)
                })?;
            current_network_name = Some(network_name);
            continue;
        }

        // Check for contract
        if let Some(caps) = contract_re.captures(line) {
            let name = caps
                .name("name")
                .map(|m| m.as_str().trim().to_string())
                .ok_or_else(|| {
                    anyhow!("Failed to capture contract name on line {}", line_num + 1)
                })?;

            let address = caps
                .name("address")
                .map(|m| m.as_str().to_string())
                .ok_or_else(|| {
                    anyhow!(
                        "Failed to capture contract address on line {}",
                        line_num + 1
                    )
                })?;

            let net_name = current_network_name.clone().ok_or_else(|| {
                anyhow!(
                    "Found contract definition before network header on line {}",
                    line_num + 1
                )
            })?;

            // Find existing network group or create new one
            if let Some(pos) = networks.iter().position(|n| n.name == net_name) {
                networks[pos].contracts.push(Contract { name, address });
            } else {
                networks.push(Network {
                    name: net_name,
                    contracts: vec![Contract { name, address }],
                });
            }
        }
    }
    Ok(networks)
}

async fn verify_network(
    networks: &[Network],
    l1_network_name: &str,
    l2_network_name: &str,
    rpc_url: Option<String>,
) -> Result<Vec<CheckResult>> {
    let rpc_url = match rpc_url {
        Some(url) => url,
        None => return Ok(vec![]),
    };

    // Fail fast if we can't find the configuration addresses needed for lookup
    let sys_config = get_addr(networks, l1_network_name, "SystemConfig")?;
    let dispute_game_factory = get_addr(networks, l1_network_name, "DisputeGameFactoryProxy")?;
    let fault_dispute_game = get_addr(networks, l1_network_name, "FaultDisputeGame")?;
    let permissioned_dispute_game = get_addr(networks, l1_network_name, "PermissionedDisputeGame")?;
    let mips = get_addr(networks, l1_network_name, "MIPS")?;

    let multicall = Multicall3::new(
        Address::from_str(MULTICALL3_ADDRESS).context("Invalid Multicall3 constant")?,
        ProviderBuilder::new().on_http(rpc_url.parse().context("Invalid RPC URL")?),
    );

    struct CheckConfig<'a> {
        name: &'a str,
        file_search_name: &'a str,
        network: &'a str,
        call_data: Vec<u8>,
        target: Address,
        decoder: Decoder,
    }

    // Helper to create a decoder
    fn make_decoder<C: SolCall>(f: fn(C::Return) -> Address) -> Decoder
    where
        C::Return: Send + Sync + 'static,
    {
        Box::new(move |data| {
            let ret = C::abi_decode_returns(data, true)?;
            Ok(f(ret))
        })
    }

    // Common decoder for simple address returns
    // Note: Most functions generated by alloy for `returns (address)` return a tuple `(Address,)`
    // or struct with field `_0`.

    let checks: Vec<CheckConfig> = vec![
        CheckConfig {
            name: "Batch Inbox",
            file_search_name: "Batch Inbox",
            network: l2_network_name,
            call_data: SystemConfig::batchInboxCall {}.abi_encode(),
            target: sys_config,
            decoder: make_decoder::<SystemConfig::batchInboxCall>(|r| r._0),
        },
        CheckConfig {
            name: "DisputeGameFactory",
            file_search_name: "DisputeGameFactoryProxy",
            network: l1_network_name,
            call_data: SystemConfig::disputeGameFactoryCall {}.abi_encode(),
            target: sys_config,
            decoder: make_decoder::<SystemConfig::disputeGameFactoryCall>(|r| r._0),
        },
        CheckConfig {
            name: "Fault Dispute Game",
            file_search_name: "FaultDisputeGame",
            network: l1_network_name,
            call_data: DisputeGameFactory::gameImplsCall { gameType: 0 }.abi_encode(),
            target: dispute_game_factory,
            decoder: make_decoder::<DisputeGameFactory::gameImplsCall>(|r| r._0),
        },
        CheckConfig {
            name: "Permissioned Dispute Game",
            file_search_name: "PermissionedDisputeGame",
            network: l1_network_name,
            call_data: DisputeGameFactory::gameImplsCall { gameType: 1 }.abi_encode(),
            target: dispute_game_factory,
            decoder: make_decoder::<DisputeGameFactory::gameImplsCall>(|r| r._0),
        },
        CheckConfig {
            name: "Challenger",
            file_search_name: "Challenger",
            network: l2_network_name,
            call_data: PermissionedDisputeGame::challengerCall {}.abi_encode(),
            target: permissioned_dispute_game,
            decoder: make_decoder::<PermissionedDisputeGame::challengerCall>(|r| r._0),
        },
        CheckConfig {
            name: "Proposer",
            file_search_name: "Output Proposer",
            network: l2_network_name,
            call_data: PermissionedDisputeGame::proposerCall {}.abi_encode(),
            target: permissioned_dispute_game,
            decoder: make_decoder::<PermissionedDisputeGame::proposerCall>(|r| r._0),
        },
        CheckConfig {
            name: "Guardian",
            file_search_name: "Guardian",
            network: l2_network_name,
            call_data: SystemConfig::guardianCall {}.abi_encode(),
            target: sys_config,
            decoder: make_decoder::<SystemConfig::guardianCall>(|r| r._0),
        },
        CheckConfig {
            name: "L1CrossDomainMessenger",
            file_search_name: "L1CrossDomainMessenger",
            network: l1_network_name,
            call_data: SystemConfig::l1CrossDomainMessengerCall {}.abi_encode(),
            target: sys_config,
            decoder: make_decoder::<SystemConfig::l1CrossDomainMessengerCall>(|r| r._0),
        },
        CheckConfig {
            name: "L1ERC721Bridge",
            file_search_name: "L1ERC721Bridge",
            network: l1_network_name,
            call_data: SystemConfig::l1ERC721BridgeCall {}.abi_encode(),
            target: sys_config,
            decoder: make_decoder::<SystemConfig::l1ERC721BridgeCall>(|r| r._0),
        },
        CheckConfig {
            name: "L1StandardBridge",
            file_search_name: "L1StandardBridge",
            network: l1_network_name,
            call_data: SystemConfig::l1StandardBridgeCall {}.abi_encode(),
            target: sys_config,
            decoder: make_decoder::<SystemConfig::l1StandardBridgeCall>(|r| r._0),
        },
        CheckConfig {
            name: "OptimismMintableERC20Factory",
            file_search_name: "OptimismMintableERC20Factory",
            network: l1_network_name,
            call_data: SystemConfig::optimismMintableERC20FactoryCall {}.abi_encode(),
            target: sys_config,
            decoder: make_decoder::<SystemConfig::optimismMintableERC20FactoryCall>(|r| r._0),
        },
        CheckConfig {
            name: "OptimismPortal",
            file_search_name: "OptimismPortal",
            network: l1_network_name,
            call_data: SystemConfig::optimismPortalCall {}.abi_encode(),
            target: sys_config,
            decoder: make_decoder::<SystemConfig::optimismPortalCall>(|r| r._0),
        },
        CheckConfig {
            name: "ProxyAdmin",
            file_search_name: "ProxyAdmin",
            network: l1_network_name,
            call_data: SystemConfig::proxyAdminCall {}.abi_encode(),
            target: sys_config,
            decoder: make_decoder::<SystemConfig::proxyAdminCall>(|r| r._0),
        },
        CheckConfig {
            name: "Proxy Admin Owner",
            file_search_name: "Proxy Admin Owner (L1)",
            network: l2_network_name,
            call_data: SystemConfig::proxyAdminOwnerCall {}.abi_encode(),
            target: sys_config,
            decoder: make_decoder::<SystemConfig::proxyAdminOwnerCall>(|r| r._0),
        },
        CheckConfig {
            name: "SystemConfig Owner",
            file_search_name: "System config owner",
            network: l2_network_name,
            call_data: SystemConfig::ownerCall {}.abi_encode(),
            target: sys_config,
            decoder: make_decoder::<SystemConfig::ownerCall>(|r| r._0),
        },
        CheckConfig {
            name: "AnchorStateRegistry",
            file_search_name: "AnchorStateRegistryProxy",
            network: l1_network_name,
            call_data: FaultDisputeGame::anchorStateRegistryCall {}.abi_encode(),
            target: fault_dispute_game,
            decoder: make_decoder::<FaultDisputeGame::anchorStateRegistryCall>(|r| r._0),
        },
        CheckConfig {
            name: "MIPS",
            file_search_name: "MIPS",
            network: l1_network_name,
            call_data: FaultDisputeGame::vmCall {}.abi_encode(),
            target: fault_dispute_game,
            decoder: make_decoder::<FaultDisputeGame::vmCall>(|r| r._0),
        },
        CheckConfig {
            name: "PreimageOracle",
            file_search_name: "PreimageOracle",
            network: l1_network_name,
            call_data: MIPS::oracleCall {}.abi_encode(),
            target: mips,
            decoder: make_decoder::<MIPS::oracleCall>(|r| r._0),
        },
        CheckConfig {
            name: "DelayedWETHProxy (FDG)",
            file_search_name: "DelayedWETHProxy (FDG)",
            network: l1_network_name,
            call_data: FaultDisputeGame::wethCall {}.abi_encode(),
            target: fault_dispute_game,
            decoder: make_decoder::<FaultDisputeGame::wethCall>(|r| r._0),
        },
        CheckConfig {
            name: "DelayedWETHProxy (PDG)",
            file_search_name: "DelayedWETHProxy (PDG)",
            network: l1_network_name,
            call_data: PermissionedDisputeGame::wethCall {}.abi_encode(),
            target: permissioned_dispute_game,
            decoder: make_decoder::<PermissionedDisputeGame::wethCall>(|r| r._0),
        },
    ];

    let mut calls = Vec::with_capacity(checks.len());
    let mut expected_addresses = Vec::with_capacity(checks.len());

    for check in &checks {
        let expected = find_contract_address(networks, check.network, check.file_search_name);
        expected_addresses.push(expected);

        calls.push(Multicall3::Call3 {
            target: check.target,
            allowFailure: true,
            callData: check.call_data.clone().into(),
        });
    }

    let result = multicall
        .aggregate3(calls)
        .call()
        .await
        .context(format!("Multicall execution failed on {}", l1_network_name))?;

    let mut check_results = Vec::new();

    for (i, check) in checks.iter().enumerate() {
        let res = &result.returnData[i];

        let result = process_result(
            check.name,
            check.network,
            expected_addresses[i].clone(),
            res,
            &check.decoder,
        );
        check_results.push(result);
    }

    Ok(check_results)
}

fn get_addr(networks: &[Network], network_name: &str, contract_name: &str) -> Result<Address> {
    let addr_str =
        find_contract_address(networks, network_name, contract_name).ok_or_else(|| {
            anyhow!(
                "Could not find {} address for {}",
                contract_name,
                network_name
            )
        })?;

    Address::from_str(&addr_str).with_context(|| {
        format!(
            "Error parsing {} address for {}",
            contract_name, network_name
        )
    })
}

fn find_contract_address(
    networks: &[Network],
    network_name: &str,
    contract_name: &str,
) -> Option<String> {
    networks
        .iter()
        .find(|n| n.name == network_name)
        .and_then(|n| {
            n.contracts
                .iter()
                .find(|c| c.name.eq_ignore_ascii_case(contract_name))
                .map(|c| c.address.clone())
        })
}

fn process_result(
    contract_name: &str,
    expected_addr_network: &str,
    expected_addr: Option<String>,
    res: &Multicall3::Result,
    decoder: &Decoder,
) -> CheckResult {
    let mut result = CheckResult {
        name: contract_name.to_string(),
        network: expected_addr_network.to_string(),
        expected: None,
        actual: None,
        success: false,
        error: None,
    };

    let expected_str = match expected_addr {
        Some(s) => s,
        None => {
            result.error = Some(format!(
                "Could not find expected address in config for {}",
                expected_addr_network
            ));
            return result;
        }
    };

    let expected = match Address::from_str(&expected_str) {
        Ok(a) => a,
        Err(e) => {
            result.error = Some(format!(
                "Error parsing expected address {}: {}",
                expected_str, e
            ));
            return result;
        }
    };
    result.expected = Some(expected);

    if !res.success {
        result.error = Some("View call failed on-chain".to_string());
        return result;
    }

    let on_chain_addr = match decoder(&res.returnData) {
        Ok(addr) => addr,
        Err(e) => {
            result.error = Some(format!("Error decoding return data: {}", e));
            return result;
        }
    };
    result.actual = Some(on_chain_addr);

    if on_chain_addr != expected {
        return result; // success is already false
    }

    result.success = true;
    result
}
