/**
 * Copyright 2022 Airwallex (Hong Kong) Limited
 *
 * Licensed under the Apache License, Version 2.0 (the "License"); you may not use this file except
 * in compliance with the License.
 *
 * You may obtain a copy of the License at
 *   http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software distributed under the License
 * is distributed on an "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express
 * or implied.
 *
 * See the License for the specific language governing permissions and limitations under the License.
 */
use crate::config::EventLogGCSetting::{Disabled, Enabled};
use tikv_util::info;

/**
 * Wallet Service Configurations
 */
#[derive(Clone)]
pub struct WalletServiceConfig {
    /// The maximum count of events that is returned by `InternalService.query_events()`
    pub query_events_max_count: u64,
}

impl WalletServiceConfig {
    /// The default value for `WalletServiceConfig.query_events_max_count`
    const DEFAULT_QUERY_EVENTS_MAX_COUNT: u64 = 1000;
}

impl Default for WalletServiceConfig {
    fn default() -> Self {
        let query_events_max_count = std::env::var("query_events_max_count").map_or(
            WalletServiceConfig::DEFAULT_QUERY_EVENTS_MAX_COUNT,
            |v| {
                v.parse::<u64>()
                    .unwrap_or(WalletServiceConfig::DEFAULT_QUERY_EVENTS_MAX_COUNT)
            },
        );
        WalletServiceConfig {
            query_events_max_count,
        }
    }
}

/**
 * Cluster Configurations
 */
pub struct ClusterConfig {
    /// The raft `cluster_id`
    pub cluster_id: u64,

    /// The number of replicas in the raft cluster
    pub cluster_size: u64,

    /// The raft `store_id`
    pub store_id: u64,
}

impl ClusterConfig {
    const DEFAULT_CLUSTER_ID: u64 = 1;
    const DEFAULT_CLUSTER_SIZE: u64 = 5;
}

impl Default for ClusterConfig {
    fn default() -> Self {
        let cluster_id: u64 =
            std::env::var("cluster_id").map_or(ClusterConfig::DEFAULT_CLUSTER_ID, |v| {
                v.parse::<u64>()
                    .unwrap_or(ClusterConfig::DEFAULT_CLUSTER_ID)
            });
        let cluster_size = std::env::var("cluster_size")
            .unwrap_or(ClusterConfig::DEFAULT_CLUSTER_SIZE.to_string())
            .parse::<u64>()
            .unwrap();
        let store_id = std::env::var("store_id").unwrap().parse::<u64>().unwrap();

        ClusterConfig {
            cluster_id,
            cluster_size,
            store_id,
        }
    }
}

/**
 * GC related Configurations
 */
pub struct EventLogGCConfig {
    /// The interval between two consecutive idle run of GC.
    pub poll_interval_millis: u64,

    /// The max num of events that GC retains, e.g, count_limit = 1,000,000 means GC retains latest
    /// 1,000,000 events.
    pub count_limit: u64,

    /// The max num of events that a single delete operation handles.
    pub batch_size: u64,

    /// The percentage of total events(in RocksDB) to be truncated when the GC threshold is reached.
    /// Example:
    /// Events range in RocksDB: [10,000,000, 60,010,000], `count_limit` = 50,000,000,
    /// `percentage` = 0.1. Since (60,010,000 - 10,000,000) > 50,000,000, then
    /// 50,000,000 * 0.1 = 5,000,000 entries will be truncated. After gc finished, the events in
    /// RocksDB will start from 10,000,000 + 5,000,000 = 15,000,000
    pub percentage: f64,
}

impl EventLogGCConfig {
    const DEFAULT_GC_POLL_INTERVAL_MILLIS: u64 = 1000;
    const DEFAULT_GC_COUNT_LIMIT: u64 = 1024 * 1024 * 50;
    const DEFAULT_GC_BATCH_SIZE: u64 = 200;
    const DEFAULT_GC_PERCENTAGE: f64 = 0.1;

    const MAX_GC_PERCENTAGE: f64 = 0.25;
    const MIN_GC_PERCENTAGE: f64 = 0.05;
}

impl Default for EventLogGCConfig {
    fn default() -> Self {
        let gc_poll_interval_millis = std::env::var("event_log_gc_poll_interval_millis").map_or(
            EventLogGCConfig::DEFAULT_GC_POLL_INTERVAL_MILLIS,
            |v| {
                v.parse::<u64>()
                    .unwrap_or(EventLogGCConfig::DEFAULT_GC_POLL_INTERVAL_MILLIS)
            },
        );
        let gc_count_limit: u64 = std::env::var("event_log_gc_count_limit").map_or(
            EventLogGCConfig::DEFAULT_GC_COUNT_LIMIT,
            |v| {
                v.parse::<u64>()
                    .unwrap_or(EventLogGCConfig::DEFAULT_GC_COUNT_LIMIT)
            },
        );
        let gc_batch_size: u64 = std::env::var("event_log_gc_batch_size").map_or(
            EventLogGCConfig::DEFAULT_GC_BATCH_SIZE,
            |v| {
                v.parse::<u64>()
                    .unwrap_or(EventLogGCConfig::DEFAULT_GC_BATCH_SIZE)
            },
        );

        let mut gc_percentage: f64 = std::env::var("event_log_gc_percentage").map_or(
            EventLogGCConfig::DEFAULT_GC_PERCENTAGE,
            |v| {
                v.parse::<f64>()
                    .unwrap_or(EventLogGCConfig::DEFAULT_GC_PERCENTAGE)
            },
        );

        if gc_percentage > EventLogGCConfig::MAX_GC_PERCENTAGE
            || gc_percentage < EventLogGCConfig::MIN_GC_PERCENTAGE
        {
            info!(
                "event_log_gc_percentage={} is not within the legal range [{}, {}], defaulting to {}",
                gc_percentage,
                EventLogGCConfig::MIN_GC_PERCENTAGE,
                EventLogGCConfig::MAX_GC_PERCENTAGE,
                EventLogGCConfig::DEFAULT_GC_PERCENTAGE
            );
            gc_percentage = EventLogGCConfig::DEFAULT_GC_PERCENTAGE;
        }

        EventLogGCConfig {
            poll_interval_millis: gc_poll_interval_millis,
            count_limit: gc_count_limit,
            batch_size: gc_batch_size,
            percentage: gc_percentage,
        }
    }
}

pub enum EventLogGCSetting {
    Disabled,
    Enabled(EventLogGCConfig),
}

impl EventLogGCSetting {
    fn gc_enabled() -> bool {
        std::env::var("enable_gc_of_event_log")
            .unwrap_or_default()
            .parse::<bool>()
            .unwrap_or(true)
    }
}

impl Default for EventLogGCSetting {
    fn default() -> Self {
        if Self::gc_enabled() {
            let config: EventLogGCConfig = EventLogGCConfig::default();
            Enabled(config)
        } else {
            Disabled
        }
    }
}

#[derive(Default)]
pub struct Config {
    pub wallet_service_config: WalletServiceConfig,

    /// Raft Cluster Config
    pub cluster_config: ClusterConfig,

    pub event_log_gc_setting: EventLogGCSetting,
}
