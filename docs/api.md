## API Documentation
### Fetch proto definitions
```
git clone https://gitlab.awx.im/financial_platform/the-hologram/hologram-protos.git

# Checkout the proto version currently used. 
# You could search "hologram_protos" in Cargo.lock to find the SHA of commit.
git checkout 683996b743a992f0cdf479d6387c55a83ad656ad

# Change directory
cd src/proto
```

### Balance Operation
The BalanceOperationService
```
service BalanceOperationService {
    rpc Transfer(TransferRequest) returns (TransferResponse) {}
}
```

#### Bilateral Transfer

**Description:**
Transfer `amount` from `from_account_id` to `to_account_id`, the `TransferResponse` contains the 
previous and current `Balance`.

- TransferRequest
```
message TransferRequest {
  string dedup_id = 1;
  TransferSpec transfer_spec = 2;

  // TransferSpec ensures debit amount == credit amount
  message TransferSpec {
    string amount = 1;
    string from_account_id = 2;
    string to_account_id = 3;
    string metadata = 4;
  }
}
```

- TransferResponse
```
message TransferResponse {
  errorpb.Error error = 1;
  commonpb.ResponseHeader header = 2;
  TransferRequest request = 3;
  BalanceChange from = 4;
  BalanceChange to = 5;
}

message BalanceChange {
  string account_id = 1;
  accountpb.Balance prev_balance = 2;
  accountpb.Balance curr_balance = 3;
}

message Balance {
  // Total balance = Available balance + Reserved balance

  string available = 1;
  map<string, string> reservations = 2; // reservation_id -> reservation amount
}
```

**Example:**
- Request

```
grpcurl -plaintext -import-path ./firm_wallet -import-path ./ -proto balance_operation_servicepb.proto -d '{"dedup_id":
 "1234567890", "transfer_spec": {"from_account_id": "tony", "to_account_id": "ben", "amount": "1234.5"}}'  127.0.0.1:20161 firm_wallet.balance_operation_servicepb.BalanceOperationService/Transfer
```

- Response

```
{
  "header": {
    "seqNum": "1",
    "logIndex": "7"
  },
  "request": {
    "dedupId": "1234567890",
    "transferSpec": {
      "amount": "1234.5",
      "fromAccountId": "tony",
      "toAccountId": "ben"
    }
  },
  "from": {
    "accountId": "tony",
    "prevBalance": {
      "available": "0.0"
    },
    "currBalance": {
      "available": "-1234.5"
    }
  },
  "to": {
    "accountId": "ben",
    "prevBalance": {
      "available": "0.0"
    },
    "currBalance": {
      "available": "1234.5"
    }
  }
}
```

### Account Operation
TBD