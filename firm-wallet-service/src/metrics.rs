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
use lazy_static::lazy_static;
use prometheus::*;
use prometheus_static_metric::*;

/**
 * Metrics
 */
const DEFAULT_BUCKETS: &[f64; 40] = &[
    0.005, 0.010, 0.015, 0.020, 0.025, 0.030, 0.035, 0.040, 0.045, 0.050, 0.055, 0.060, 0.065,
    0.070, 0.075, 0.080, 0.085, 0.090, 0.095, 0.100, 0.110, 0.120, 0.130, 0.140, 0.150, 0.160,
    0.170, 0.180, 0.190, 0.200, 0.210, 0.220, 0.230, 0.240, 0.250, 0.260, 0.270, 0.280, 0.290,
    0.300,
];

make_auto_flush_static_metric! {
    pub label_enum LatencyKind {
        transfer,
        reserve,
        release,
        batch_balance_operation
    }

    pub struct LatencyHistogramVec: LocalHistogram {
        "type" => LatencyKind,
    }
}

lazy_static! {
    pub static ref LATENCY_HISTOGRAM_VEC: HistogramVec = register_histogram_vec!(
        "wallet_latency_duration_seconds",
        "Bucketed histogram of wallet internal latency",
        &["type"],
        // exponential_buckets(0.0005, 2.0, 20).unwrap()
        DEFAULT_BUCKETS.to_vec()
    )
    .unwrap();
    pub static ref LATENCY_HISTOGRAM_STATIC: LatencyHistogramVec =
        auto_flush_from!(LATENCY_HISTOGRAM_VEC, LatencyHistogramVec);
}

const CRITICAL_BUCKETS: &[f64; 40] = &[
    0.000005, 0.000010, 0.000015, 0.000020, 0.000025, 0.000030, 0.000035, 0.000040, 0.000045,
    0.000050, 0.000055, 0.000060, 0.000065, 0.000070, 0.000075, 0.000080, 0.000085, 0.000090,
    0.000095, 0.000100, 0.000110, 0.000120, 0.000130, 0.000140, 0.000150, 0.000160, 0.000170,
    0.000180, 0.000190, 0.000200, 0.000210, 0.000220, 0.000230, 0.000240, 0.000250, 0.000260,
    0.000270, 0.000280, 0.000290, 0.000300,
];

make_auto_flush_static_metric! {
    pub label_enum CriticalEventType {
        read_event_from_db,
        read_seq_num_from_db,
        handle_command,
    }

    pub struct CriticalEventHistogramVec: LocalHistogram {
        "type" => CriticalEventType,
    }
}

lazy_static! {
    pub static ref CRITICAL_EVENT_HISTOGRAM_VEC: HistogramVec = register_histogram_vec!(
        "critical_event_duration_seconds",
        "Bucketed histogram of critical event",
        &["type"],
        // exponential_buckets(0.0005, 2.0, 20).unwrap()
        CRITICAL_BUCKETS.to_vec()
    ).unwrap();
    pub static ref CRITICAL_EVENT_HISTOGRAM_STATIC: CriticalEventHistogramVec =
        auto_flush_from!(CRITICAL_EVENT_HISTOGRAM_VEC, CriticalEventHistogramVec);

    pub static ref APPLY_COMMAND_COUNTER: IntCounter = register_int_counter!(
        "apply_command_counter",
        "Number of applied commands"
    ).unwrap();
}
