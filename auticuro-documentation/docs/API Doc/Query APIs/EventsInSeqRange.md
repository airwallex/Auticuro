---
sidebar_position: 5
---

# Events In Seq Range
**`EventsInSeqRange(start_seq_num: u64, end_seq_num: u64)`**

### Description
`EventsInSeqRange` retrieves all the events with consecutive sequence number in range of _[start_seq_num, end_seq_num)_.
Partial results are returned if the input seq_num is out of system's available range.

### Definitions

Events in seq range response:

```protobuf3
message EventsInSeqRangeResponse {
   option Error error = 1;
   uint64 start_seq_num = 2;
   uint64 end_seq_num = 3;
   repeated Event events = 4;
}
```

Note: for `Event` definition, see `firm_wallet/internal_servicepb.proto:Event`