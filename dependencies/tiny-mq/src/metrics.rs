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

use prometheus::*;
use prometheus_static_metric::*;

use lazy_static::lazy_static;
use tikv_util::warn;

const DEFAULT_BUCKETS: &[f64; 40] = &[
    0.005, 0.010, 0.015, 0.020, 0.025, 0.030, 0.035, 0.040, 0.045, 0.050,
    0.055, 0.060, 0.065, 0.070, 0.075, 0.080, 0.085, 0.090, 0.095, 0.100,
    0.110, 0.120, 0.130, 0.140, 0.150, 0.160, 0.170, 0.180, 0.190, 0.200,
    0.210, 0.220, 0.230, 0.240, 0.250, 0.260, 0.270, 0.280, 0.290, 0.300,
];

make_auto_flush_static_metric! {
    // Todo Ideally we should change GrpcTypeKind => LatencyTypeKind, but that would lead to lots of
    // changes in grafana dashboard, we may fix the naming later.
    pub label_enum GrpcTypeKind {
        query_raft_log_entries,
        message_queue,
        log_consumer_queueing,
    }

    pub struct GrpcMsgHistogramVec: LocalHistogram {
        "type" => GrpcTypeKind,
    }
}

lazy_static! {
    pub static ref GRPC_MSG_HISTOGRAM_VEC: HistogramVec = register_histogram_vec!(
        "tikv_grpc_msg_duration_seconds",
        "Bucketed histogram of grpc server messages",
        &["type"],
        // exponential_buckets(0.0005, 2.0, 20).unwrap()
        DEFAULT_BUCKETS.to_vec()
    )
    .unwrap();
}

lazy_static! {
    pub static ref GRPC_MSG_HISTOGRAM_STATIC: GrpcMsgHistogramVec =
        auto_flush_from!(GRPC_MSG_HISTOGRAM_VEC, GrpcMsgHistogramVec);
}

lazy_static! {
    pub static ref APPLY_ENTRY_COUNTER: IntCounter =
        register_int_counter!("apply_entry_counter", "Number of entries applied").unwrap();
    pub static ref APPLY_EVENT_COUNTER: IntCounter =
        register_int_counter!("apply_event_counter", "Number of events applied").unwrap();
    pub static ref IS_LEADER_GAUGE: IntGauge =
        register_int_gauge!("is_leader_gauge", "Is current node a raft leader").unwrap();
}

pub fn dump() -> String {
    let mut buffer = vec![];
    let encoder = TextEncoder::new();
    let metric_families = prometheus::gather();
    for mf in metric_families {
        if let Err(e) = encoder.encode(&[mf], &mut buffer) {
            warn!("prometheus encoding error"; "err" => ?e);
        }
    }
    String::from_utf8(buffer).unwrap()
}
