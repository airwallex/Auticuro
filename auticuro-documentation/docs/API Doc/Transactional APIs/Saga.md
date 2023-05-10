---
sidebar_position: 1
---

# Saga
Auticuro support Saga transaction by providing the Compensate API, which will be called by the
transaction manager.

The input:
- _dedup_id_: the dedup id of the original request

The request handling:
- Search the original request by the dedup id, if found, compensate the original request and set 
the compensation state as `COMPENSATE_SUCCESSFULLY`
- Else, record the compensation state as `NULL_COMPENSATE`, the original request will be ignored if 
it comes later


```protobuf
message CompensateRequest {
  string dedup_id = 1; // dedup id of original Request
  string metadata = 2;
}

message CompensateResponse {
  errorpb.Error error = 1;
  commonpb.ResponseHeader header = 2;
  CompensateRequest request = 3;
  CompensationState compensation_state = 4;
}

enum CompensationState {
  COMPENSATE_SUCCESSFULLY = 0;
  NULL_COMPENSATE = 1;
}
```