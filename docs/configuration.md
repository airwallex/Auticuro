# Configuration Guide

#### Wallet Service Configuration
| Wallet Service configuration |                  Description                   |  Default Value  |
|:----------------------------:|:----------------------------------------------:|:---------------:|
|    query_events_max_count    | Max queried events count of QueryEventsRequest |      1000       |

- Tuning the Wallet Service Configuration

  `export query_events_max_count=100`


#### GC Configuration
The wallet's earliest entries(by seq_num) will be truncated if the GC is enabled and exceeds the `event_log_gc_count_limit`, currently
the GC is disabled by default.

- How to enable GC

  ```export enable_gc_of_event_log=true```


- Tuning the GC configurations

  Please remember to export gc configuration with gc **enabled**.

  ```
   export enable_gc_of_event_log=true
   export event_log_gc_count_limit=10000
  ```

|             GC configuration              |                                     Description                                     | Default Value |
|:-----------------------------------------:|:-----------------------------------------------------------------------------------:|:-------------:|
| event_log_gc_poll_interval_millis(millis) |                  The poll interval if gc thread find gc not needed                  |     1000      |
|       event_log_gc_count_limit(u64)       |                     Max entries that can be stored in rocks db                      |  50 million   |
|       event_log_gc_batch_size(u64)        |                   Num of seq_nums to be deleted in a delete batch                   |      200      |
|       event_log_gc_percentage(f64)        | Each time event_log_gc_percentage * event_log_gc_count_limit events will be deleted |      0.1      |
