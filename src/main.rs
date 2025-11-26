use alloy::primitives::Address;
use alloy::providers::ProviderBuilder;
use alloy::sol_types::{SolCall, SolValue};
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

const MULTICALL3_ADDRESS: &str = "0xcA11bde05977b3631167028862bE2a173976CA11";

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    /// Path to the file to parse
    #[arg(short, long, value_name = "FILE")]
    file: PathBuf,

    /// Mainnet RPC URL
    #[arg(long, value_name = "URL")]
    mainnet_rpc_url: Option<String>,

    /// Sepolia RPC URL
    #[arg(long, value_name = "URL")]
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
        "Ethereum Mainnet",
        "Base Mainnet",
        cli.mainnet_rpc_url,
    );

    let sepolia_task = verify_network(
        &networks,
        "Ethereum Testnet (Sepolia)",
        "Base Testnet (Sepolia)",
        cli.sepolia_rpc_url,
    );

    let (mainnet_res, sepolia_res) = tokio::join!(mainnet_task, sepolia_task);

    // Check for RPC/System errors
    let mainnet_success = mainnet_res.context("Mainnet verification failed during execution")?;
    let sepolia_success = sepolia_res.context("Sepolia verification failed during execution")?;

    if !mainnet_success || !sepolia_success {
        eprintln!("\n❌ Verification failed for one or more networks.");
        std::process::exit(1);
    }

    println!("\n✅ All checks passed successfully.");
    Ok(())
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
                .ok_or_else(|| anyhow!("Failed to capture network name on line {}", line_num))?;
            current_network_name = Some(network_name);
            continue;
        }

        // Check for contract
        if let Some(caps) = contract_re.captures(line) {
            let name = caps
                .name("name")
                .map(|m| m.as_str().trim().to_string())
                .unwrap_or_else(|| "Unknown".to_string());
            let address = caps
                .name("address")
                .map(|m| m.as_str().to_string())
                .unwrap_or_else(|| "Unknown".to_string());

            let net_name = current_network_name.clone().ok_or_else(|| {
                anyhow!(
                    "Found contract definition before network header on line {}",
                    line_num
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
) -> Result<bool> {
    let rpc_url = match rpc_url {
        Some(url) => url,
        None => {
            println!(
                "Skipping verification for {} (No RPC URL provided)",
                l1_network_name
            );
            return Ok(true);
        }
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
    }

    let checks = vec![
        CheckConfig {
            name: "Batch Inbox",
            file_search_name: "Batch Inbox",
            network: l2_network_name,
            call_data: SystemConfig::batchInboxCall {}.abi_encode(),
            target: sys_config,
        },
        CheckConfig {
            name: "DisputeGameFactory",
            file_search_name: "DisputeGameFactoryProxy",
            network: l1_network_name,
            call_data: SystemConfig::disputeGameFactoryCall {}.abi_encode(),
            target: sys_config,
        },
        CheckConfig {
            name: "Fault Dispute Game",
            file_search_name: "FaultDisputeGame",
            network: l1_network_name,
            call_data: DisputeGameFactory::gameImplsCall { gameType: 0 }.abi_encode(),
            target: dispute_game_factory,
        },
        CheckConfig {
            name: "Permissioned Dispute Game",
            file_search_name: "PermissionedDisputeGame",
            network: l1_network_name,
            call_data: DisputeGameFactory::gameImplsCall { gameType: 1 }.abi_encode(),
            target: dispute_game_factory,
        },
        CheckConfig {
            name: "Challenger",
            file_search_name: "Challenger",
            network: l2_network_name,
            call_data: PermissionedDisputeGame::challengerCall {}.abi_encode(),
            target: permissioned_dispute_game,
        },
        CheckConfig {
            name: "Proposer",
            file_search_name: "Output Proposer",
            network: l2_network_name,
            call_data: PermissionedDisputeGame::proposerCall {}.abi_encode(),
            target: permissioned_dispute_game,
        },
        CheckConfig {
            name: "Guardian",
            file_search_name: "Guardian",
            network: l2_network_name,
            call_data: SystemConfig::guardianCall {}.abi_encode(),
            target: sys_config,
        },
        CheckConfig {
            name: "L1CrossDomainMessenger",
            file_search_name: "L1CrossDomainMessenger",
            network: l1_network_name,
            call_data: SystemConfig::l1CrossDomainMessengerCall {}.abi_encode(),
            target: sys_config,
        },
        CheckConfig {
            name: "L1ERC721Bridge",
            file_search_name: "L1ERC721Bridge",
            network: l1_network_name,
            call_data: SystemConfig::l1ERC721BridgeCall {}.abi_encode(),
            target: sys_config,
        },
        CheckConfig {
            name: "L1StandardBridge",
            file_search_name: "L1StandardBridge",
            network: l1_network_name,
            call_data: SystemConfig::l1StandardBridgeCall {}.abi_encode(),
            target: sys_config,
        },
        CheckConfig {
            name: "OptimismMintableERC20Factory",
            file_search_name: "OptimismMintableERC20Factory",
            network: l1_network_name,
            call_data: SystemConfig::optimismMintableERC20FactoryCall {}.abi_encode(),
            target: sys_config,
        },
        CheckConfig {
            name: "OptimismPortal",
            file_search_name: "OptimismPortal",
            network: l1_network_name,
            call_data: SystemConfig::optimismPortalCall {}.abi_encode(),
            target: sys_config,
        },
        CheckConfig {
            name: "ProxyAdmin",
            file_search_name: "ProxyAdmin",
            network: l1_network_name,
            call_data: SystemConfig::proxyAdminCall {}.abi_encode(),
            target: sys_config,
        },
        CheckConfig {
            name: "Proxy Admin Owner",
            file_search_name: "Proxy Admin Owner (L1)",
            network: l2_network_name,
            call_data: SystemConfig::proxyAdminOwnerCall {}.abi_encode(),
            target: sys_config,
        },
        CheckConfig {
            name: "SystemConfig Owner",
            file_search_name: "System config owner",
            network: l2_network_name,
            call_data: SystemConfig::ownerCall {}.abi_encode(),
            target: sys_config,
        },
        CheckConfig {
            name: "AnchorStateRegistry",
            file_search_name: "AnchorStateRegistryProxy",
            network: l1_network_name,
            call_data: FaultDisputeGame::anchorStateRegistryCall {}.abi_encode(),
            target: fault_dispute_game,
        },
        CheckConfig {
            name: "MIPS",
            file_search_name: "MIPS",
            network: l1_network_name,
            call_data: FaultDisputeGame::vmCall {}.abi_encode(),
            target: fault_dispute_game,
        },
        CheckConfig {
            name: "PreimageOracle",
            file_search_name: "PreimageOracle",
            network: l1_network_name,
            call_data: MIPS::oracleCall {}.abi_encode(),
            target: mips,
        },
        CheckConfig {
            name: "DelayedWETHProxy (FDG)",
            file_search_name: "DelayedWETHProxy (FDG)",
            network: l1_network_name,
            call_data: FaultDisputeGame::wethCall {}.abi_encode(),
            target: fault_dispute_game,
        },
        CheckConfig {
            name: "DelayedWETHProxy (PDG)",
            file_search_name: "DelayedWETHProxy (PDG)",
            network: l1_network_name,
            call_data: PermissionedDisputeGame::wethCall {}.abi_encode(),
            target: permissioned_dispute_game,
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

    let mut all_checks_passed = true;
    for (i, check) in checks.iter().enumerate() {
        let passed = process_result(
            l1_network_name,
            check.network,
            check.name,
            expected_addresses[i].clone(),
            &result.returnData[i],
        );
        if !passed {
            all_checks_passed = false;
        }
    }

    if all_checks_passed {
        println!("✅ All addresses match for {}", l1_network_name);
    }

    Ok(all_checks_passed)
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
    l1_network: &str,
    expected_addr_network: &str,
    contract_name: &str,
    expected_addr: Option<String>,
    res: &Multicall3::Result,
) -> bool {
    let expected_str = match expected_addr {
        Some(s) => s,
        None => {
            println!(
                "Could not find expected address for {} in {}",
                contract_name, expected_addr_network
            );
            return false;
        }
    };

    let expected = match Address::from_str(&expected_str) {
        Ok(a) => a,
        Err(e) => {
            println!(
                "Error parsing expected address for {} ({}): {}",
                contract_name, expected_str, e
            );
            return false;
        }
    };

    if !res.success {
        println!("{} view call failed on {}", contract_name, l1_network);
        return false;
    }

    // Decode address from return data generically
    // Most of these calls return a single Address
    let on_chain_addr = match <Address>::abi_decode(&res.returnData, true) {
        Ok(addr) => addr,
        Err(e) => {
            println!(
                "Error decoding {} return data on {}: {}",
                contract_name, l1_network, e
            );
            return false;
        }
    };

    if on_chain_addr != expected {
        println!(
            "❌ MISMATCH for {}: \n\tFile {}: {}\n\tChain {}: {}",
            l1_network, contract_name, expected, contract_name, on_chain_addr
        );
        return false;
    }

    true
}
