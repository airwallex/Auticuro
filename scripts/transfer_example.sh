#set -x

cd dependencies/hologram-protos/src/proto

# Create Accounts through firm-wallet-gateway (127.0.0.1:20171)
echo "================================"
echo "Create an account for ben"
echo "================================"
grpcurl -plaintext -import-path ./firm_wallet -import-path ./ -proto account_management_servicepb.proto -d '{"account_config":{"asset_class":{"cash":{"currency":"USD"}}},"header":{"account_id":"ben","dedup_id":"asjdh78y"}}'  127.0.0.1:20171 firm_wallet.account_management_servicepb.AccountManagementService/CreateAccount

echo "================================"
echo "Create an account for tony"
echo "================================"
grpcurl -plaintext -import-path ./firm_wallet -import-path ./ -proto account_management_servicepb.proto -d '{"account_config":{"asset_class":{"cash":{"currency":"USD"}}},"header":{"account_id":"tony","dedup_id":"gklfjg8937"}}'  127.0.0.1:20171 firm_wallet.account_management_servicepb.AccountManagementService/CreateAccount

# Transfer money
echo "================================"
echo "Transfer 100 from tony to ben"
echo "================================"
grpcurl -plaintext -import-path ./firm_wallet -import-path ./ -proto balance_operation_servicepb.proto -d '{"dedup_id":"1234567890", "transfer_spec": {"from_account_id": "tony", "to_account_id": "ben", "amount": "100"}}'  127.0.0.1:20171 firm_wallet.balance_operation_servicepb.BalanceOperationService/Transfer

sleep 1
# Query balance and balance change events from peer_1 of Auticuro cluster (127.0.0.1:20161)
echo "================================"
echo "Query balance of tony"
echo "================================"
grpcurl -plaintext -import-path ./firm_wallet -import-path ./ -proto balance_operation_servicepb.proto -d '{"account_id": "tony"}'  127.0.0.1:20161 firm_wallet.balance_operation_servicepb.BalanceOperationService/QueryBalance

echo "================================"
echo "Query balance of ben"
echo "================================"
grpcurl -plaintext -import-path ./firm_wallet -import-path ./ -proto balance_operation_servicepb.proto -d '{"account_id": "ben"}'  127.0.0.1:20161 firm_wallet.balance_operation_servicepb.BalanceOperationService/QueryBalance

echo "================================"
echo "Query Events"
echo "================================"
grpcurl -plaintext -import-path ./firm_wallet -import-path ./ -proto internal_servicepb.proto  -d '{"first_seq_num": 1, "last_seq_num": 10}'  127.0.0.1:20161 firm_wallet.internal_servicepb.InternalService/QueryEvents
