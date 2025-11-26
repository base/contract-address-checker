use alloy::sol;

sol! {
    #[sol(rpc)]
    interface SystemConfig {
        function batchInbox() external view returns (address);
        function disputeGameFactory() external view returns (address);
        function guardian() external view returns (address);
        function l1CrossDomainMessenger() external view returns (address);
        function l1ERC721Bridge() external view returns (address);
        function l1StandardBridge() external view returns (address);
        function optimismMintableERC20Factory() external view returns (address);
        function optimismPortal() external view returns (address);
        function owner() external view returns (address);
        function proxyAdmin() external view returns (address);
        function proxyAdminOwner() external view returns (address);
    }

    #[sol(rpc)]
    interface DisputeGameFactory {
        function gameImpls(uint32 gameType) external view returns (address);
    }

    #[sol(rpc)]
    interface FaultDisputeGame {
        function anchorStateRegistry() external view returns (address);
        function vm() external view returns (address);
        function weth() external view returns (address);
    }

    #[sol(rpc)]
    interface PermissionedDisputeGame {
        function challenger() external view returns (address);
        function proposer() external view returns (address);
        function weth() external view returns (address);
    }

    #[sol(rpc)]
    interface MIPS {
        function oracle() external view returns (address);
    }

    #[sol(rpc)]
    interface Multicall3 {
        struct Call3 {
            address target;
            bool allowFailure;
            bytes callData;
        }

        struct Result {
            bool success;
            bytes returnData;
        }

        function aggregate3(Call3[] calldata calls) external view returns (Result[] memory returnData);
    }
}
