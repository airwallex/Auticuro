## API Documentation

### Balance Operation
The BalanceOperationService
```
service BalanceOperationService {
    rpc Transfer(TransferRequest) returns (TransferResponse) {}
}
```

#### Bilateral Transfer

**Description:**

Transfer `amount` from `from_account` to `to_account`, the `TransferResponse` contains the 
previous and current `Balance`.

- TransferRequest
```
message TransferRequest {
  string dedup_id = 1;     // For idempotency
  TransferSpec transfer_spec = 2;

  // TransferSpec ensures debit amount == credit amount
  message TransferSpec {
    string amount = 1;    // The amount will be transferred from the from_account to to_account
    string from_account = 2;                            
    string to_account = 3;                              
    string metadata = 4;                                
  }
}
```

- TransferResponse
```
message TransferResponse {
  Error error = 1;
  ResponseHeader header = 2;
  TransferRequest request = 3;
  BalanceChange from = 4;
  BalanceChange to = 5;
}

message BalanceChange {
  string account = 1;
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
 "1234567890", "transfer_spec": {"from_account": "tony", "to_account": "ben", "amount": "1234.5"}}'  127.0.0.1:20161 firm_wallet.balance_operation_servicepb.BalanceOperationService/Transfer
```

- Response

```
{
  "header": {
    "seqNum": "8936626",
    "logIndex": "1112234"
  },
  "request": {
    "dedupId": "1234567890",
    "transferSpec": {
      "amount": "1234.5",
      "fromAccount": "tony",
      "toAccount": "glen"
    }
  },
  "from": {
    "account": "tony",
    "prevBalance": {
      "available": "0.0"
    },
    "currBalance": {
      "available": "-1234.5"
    }
  },
  "to": {
    "account": "glen",
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