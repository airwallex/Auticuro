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
use crate::errors::common_error::Error as CommonError;
use crate::errors::general_error::GeneralResult;
use crate::utils;
use futures::channel::oneshot;
use hologram_protos::firm_walletpb::internal_servicepb::Event;
use std::borrow::BorrowMut;
use std::collections::BTreeMap;
use std::sync::mpsc;
use tikv_util::time::{duration_to_ms, Instant};
use tikv_util::{debug, error, info, trace};

#[derive(Debug, PartialEq, Eq, Hash, Clone, Copy, Ord, PartialOrd)]
pub struct Position {
    pub log_index: u64,
    pub offset: u64,
}

#[derive(Debug)]
pub struct InflightEvent {
    pub position: Position,
    pub event: GeneralResult<Event>,
}

#[derive(Debug)]
pub struct InflightRequest {
    pub position: Position,
    pub resp_to: oneshot::Sender<InflightEvent>,
    pub send_time: Instant,
}

pub struct MessageBroker {
    inflight_requests: BTreeMap<Position, (Instant, oneshot::Sender<InflightEvent>)>,
    inflight_events: BTreeMap<Position, InflightEvent>,
    inflight_request_rx: mpsc::Receiver<InflightRequest>,
    inflight_event_rx: mpsc::Receiver<InflightEvent>,
}

impl MessageBroker {
    pub fn new(
        inflight_request_rx: mpsc::Receiver<InflightRequest>,
        inflight_event_rx: mpsc::Receiver<InflightEvent>,
    ) -> Self {
        MessageBroker {
            inflight_requests: BTreeMap::new(),
            inflight_events: BTreeMap::new(),
            inflight_request_rx,
            inflight_event_rx,
        }
    }

    pub fn run(&mut self) {
        info!("Starting the message broker...");
        loop {
            let start_time = Instant::now_coarse();
            let inflight_requests = utils::batch_recv(&self.inflight_request_rx, 1000);

            for inflight_request in inflight_requests {
                match self.inflight_events.remove(&inflight_request.position) {
                    Some(inflight_event) => {
                        let _ = inflight_request
                            .resp_to
                            .send(inflight_event)
                            .map_err(|event| {
                                error!(
                                    "Message broker failed to send inflight_event: {:?} back",
                                    event
                                );
                            });
                        trace!(
                            "Position(log_index={}, offset={}): took {}ms to join the apply result",
                            inflight_request.position.log_index,
                            inflight_request.position.offset,
                            duration_to_ms(inflight_request.send_time.elapsed())
                        );
                    }
                    None => {
                        self.register_inflight_request(inflight_request);
                    }
                }
            }

            let inflight_events = utils::batch_recv(&self.inflight_event_rx, 1000);

            for inflight_event in inflight_events {
                match self.inflight_requests.remove(&inflight_event.position) {
                    Some((send_time, sender)) => {
                        let position = inflight_event.position;
                        let _ = sender.send(inflight_event).map_err(|event| {
                            error!(
                                "Message broker failed to send inflight_event: {:?} back",
                                event
                            );
                        });

                        trace!(
                            "Position(log_index={}, offset={}): took {}ms to join the apply result",
                            position.log_index,
                            position.offset,
                            duration_to_ms(send_time.elapsed())
                        );
                    }
                    None => {
                        self.register_inflight_event(inflight_event);
                    }
                }
            }

            self.gc_inflight_events();
            self.gc_inflight_requests();

            // yield CPU, avoid busy wait
            utils::yield_cpu(start_time, 50);
        }
    }

    fn gc_inflight_events(&mut self) {
        let event_gc_limit = 10000;
        while self.inflight_events.len() > event_gc_limit {
            if let Some((_position, inflight_event)) = self.inflight_events.pop_first() {
                debug!(
                    "inflight event: {:?} failed to join inflight request",
                    inflight_event
                );
            }
        }
    }

    fn gc_inflight_requests(&mut self) {
        let now = Instant::now();
        let max_timeout_ms = 1000;

        while let Some((_, (send_time, _))) = self.inflight_requests.first_key_value() {
            if now.duration_since(send_time.clone()).as_millis() < max_timeout_ms {
                break;
            }
            let (position, (_, sender)) = self.inflight_requests.pop_first().unwrap();
            let inflight_event = InflightEvent {
                position,
                event: Err(CommonError::Timeout.into()),
            };
            let _ = sender.send(inflight_event).map_err(|event| {
                error!(
                    "Message broker failed to send inflight_event: {:?} back",
                    event
                );
            });
        }
    }

    fn register_inflight_event(&mut self, inflight_event: InflightEvent) {
        self.inflight_events
            .insert(inflight_event.position, inflight_event);
    }

    fn register_inflight_request(&mut self, inflight_request: InflightRequest) {
        let InflightRequest {
            position,
            resp_to,
            send_time,
        } = inflight_request;
        self.inflight_requests
            .borrow_mut()
            .insert(position, (send_time, resp_to));
    }
}
