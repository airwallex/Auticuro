---
sidebar_position: 2
---

# TCC 
Try, Commit, and Cancel (TCC) is a model of compensating transactions. This model requires each service
of an application to provide three interfaces, that is, the try, commit, and cancel interfaces. 
The core idea of this model is to release the locking of resources at the quickest possible time by 
reserving the resources (providing intermediate states). If the transaction can be committed, the 
reserved resources are confirmed. If the transaction needs to be rolled back, the reserved resources are released.

The TCC can be divided into 3 parts
- Try: attempts to execute, completes all business checks (consistency), reserves necessary 
business resources(for quasi-isolation)
- Confirm: if all branches succeed in the Try phase, then we move to the Confirm phase, 
  where Confirm actually executes the business without any business checks, using only the business 
- Resources reserved in the Try phase
- Cancel: If one of the Trys in all branches fails, we go to the Cancel phase, which 
  releases the business resources reserved in the Try phase

### Tcc interfaces
Currently the `BatchBalanceOperation` interface is supported in TCC mode. TCC Transactions among 
multiple Auticuro shards can be achieved by using this `BatchBalanceOperation`(TCC version) 
interface.

```protobuf
  rpc TccTry(TccTryRequest) returns (TccTryResponse) {}
  rpc TccConfirm(TccConfirmRequest) returns (TccConfirmResponse) {}
  rpc TccCancel(TccCancelRequest) returns (TccCancelResponse) {}
```

### Try
For each account in the `batch_balance_operation`, checks include:
- The account is in `Normal` state
- The account balance after the request handling is within the _[lower limit, upper limit]_

If all pre-checks pass:
- For money increasing operations, add an entry: _txn_id -> amount_ in the _pending_in_ map of the account
- For money decreasing operations, moving the amount of money from _avaliable_ to an entry 
  _txn_id -> amount_ in 
  the 
  _pending_out_ map of the account

```protobuf
message TccTryRequest {
  string txn_id = 1;
  oneof request {
      TccTryBatchBalanceOperationRequest batch_balance_operation = 2;
  }
  string metadata = 3;
}

message TccTryResponse {
  TccTryRequest request = 1;
  commonpb.ResponseHeader header = 2;
  errorpb.Error error = 3;

  oneof response {
    TccTryBatchBalanceOperationResponse batch_balance_operation = 4;
  }
}
```

### Confirm
Request handling:
- Search the entry by _txn_id_ in the _pending_out_ or _pending_in_ map of the account
- If the entry is found in _pending_out_, remove the entry
- If the entry is found in _pending_in_, move the `amount` from the entry to _available_

```protobuf
message TccConfirmRequest {
  string txn_id = 1;
  string dedup_id = 2;
  string metadata = 3;
}

message TccConfirmResponse {
  TccConfirmRequest request = 1;
  commonpb.ResponseHeader header = 2;
  errorpb.Error error = 3;

  oneof response {
    TccConfirmBatchBalanceOperationResponse batch_balance_operation = 4;
  }
}
```

### Cancel
Request handling:
- Search the entry by _txn_id_ in the _pending_out_ or _pending_in_ map of the account
- If the entry is found in _pending_out_, move the `amount` from the found entry to _available_
- If the entry is found in _pending_in_, remove the entry

```protobuf
message TccCancelRequest {
  string txn_id = 1;
  string dedup_id = 2;
  string metadata = 3;
}

message TccCancelResponse {
  TccCancelRequest request = 1;
  commonpb.ResponseHeader header = 2;
  errorpb.Error error = 3;

  oneof response {
    TccCancelBatchBalanceOperationResponse batch_balance_operation = 4;
  }
}
```