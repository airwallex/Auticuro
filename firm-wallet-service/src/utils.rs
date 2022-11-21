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
use std::sync::mpsc;
use std::sync::mpsc::TryRecvError;
use std::thread;
use tikv_util::time::{duration_to_ms, Duration, Instant};
use tikv_util::trace;

pub fn batch_recv<T>(rx: &mpsc::Receiver<T>, max_count: usize) -> Vec<T> {
    let start = Instant::now();
    let mut buffer = vec![];
    while buffer.len() < max_count {
        match rx.try_recv() {
            Ok(entry) => {
                buffer.push(entry);
            }
            Err(TryRecvError::Empty) => {
                break;
            }
            Err(TryRecvError::Disconnected) => {
                unreachable!()
            }
        }
    }
    if buffer.len() > 0 {
        trace!(
            "utils::batch_recv received {} entries of type {} in {:?}ms",
            buffer.len(),
            std::any::type_name::<T>(),
            duration_to_ms(start.elapsed())
        );
    }
    buffer
}

// yield CPU for ${interval_in_us} us, avoid busy wait
pub fn yield_cpu(start_time: Instant, interval_in_us: u64) {
    let elapsed_in_us = start_time.elapsed().as_micros() as u64;
    if elapsed_in_us < interval_in_us {
        thread::sleep(Duration::from_micros(interval_in_us - elapsed_in_us));
    }
}
