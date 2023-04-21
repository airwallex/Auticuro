---
sidebar_position: 6
---

# Events In Time Range

**`EventsInTimeRange(start_timestamp: Timestamp, end_timestamp: Timestamp)`**

### Description
`EventsInTimeRange` retrieves all the events with consecutive sequence number in between *[start_timestamp, end_timestamp)*.
Partial results are returned if the input timestamp is out of system's available range. Input timestamp supports
human-readable time with timezone as well as Unix Epoch. 

### Definitions

Events in seq range response:

```protobuf3
message EventsInSeqRangeResponse {
   option Error error = 1;
   uint64 start_timestamp = 2;
   uint64 end_timestamp = 3;
   repeated Event events = 4;
}
```

Note: for `Event` definition, see `firm_wallet/internal_servicepb.proto:Event`