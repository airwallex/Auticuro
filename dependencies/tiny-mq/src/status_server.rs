#![allow(warnings)]

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

use crate::errors::Result;
use crate::metrics::dump;
use tikv_util::{info, warn};
use tikv_util::logger::{set_log_level, log_level_serde};

use std::error::Error as StdError;
use std::net::SocketAddr;
use std::str::FromStr;
use std::time::Duration;

use futures::future::{ok, poll_fn};
use futures::prelude::*;
use hyper::server::conn::AddrIncoming;
use hyper::server::Builder as HyperBuilder;
use hyper::service::{make_service_fn, service_fn};
use hyper::{self, Body, Method, Request, Response, Server, StatusCode};

use tokio::runtime::{Builder, Runtime};
use tokio::sync::oneshot::{Receiver, Sender};
use serde::{Serialize, Deserialize};

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct LogLevelRequest {
    #[serde(with = "log_level_serde")]
    pub log_level: slog::Level,
}

pub struct StatusServer {
    thread_pool: Runtime,
    tx: Sender<()>,
    rx: Option<Receiver<()>>,
    addr: Option<SocketAddr>,
}

impl StatusServer {
    pub fn new(status_thread_pool_size: usize) -> Result<Self> {
        let thread_pool = Builder::new_multi_thread()
            .enable_all()
            .worker_threads(status_thread_pool_size)
            .thread_name("status-server")
            .on_thread_start(|| {
                info!("Status server started");
            })
            .on_thread_stop(|| {
                info!("stopping status server");
            })
            .build()
            .unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        Ok(StatusServer {
            thread_pool,
            tx,
            rx: Some(rx),
            addr: None,
        })
    }

    async fn change_log_level(req: Request<Body>) -> hyper::Result<Response<Body>> {
        let mut body = Vec::new();
        req.into_body()
            .try_for_each(|bytes| {
                body.extend(bytes);
                ok(())
            })
            .await?;

        let log_level_request: std::result::Result<LogLevelRequest, serde_json::error::Error> =
            serde_json::from_slice(&body);

        match log_level_request {
            Ok(req) => {
                set_log_level(req.log_level);
                Ok(Response::new(Body::empty()))
            }
            Err(err) => Ok(StatusServer::err_response(
                StatusCode::BAD_REQUEST,
                err.to_string(),
            )),
        }
    }

    pub fn stop(self) {
        let _ = self.tx.send(());
        self.thread_pool.shutdown_timeout(Duration::from_secs(10));
    }

    // Return listening address, this may only be used for outer test
    // to get the real address because we may use "127.0.0.1:0"
    // in test to avoid port conflict.
    pub fn listening_addr(&self) -> SocketAddr {
        self.addr.unwrap()
    }

    fn start_serve(&mut self, builder: HyperBuilder<AddrIncoming>) {
        // Start to serve.
        let server = builder.serve(make_service_fn(move |_conn| {
            async move {
                // Create a status service.
                Ok::<_, hyper::Error>(service_fn(move |req: Request<Body>| async move {
                    let path = req.uri().path().to_owned();
                    let method = req.method().to_owned();

                    match (method, path.as_ref()) {
                        (Method::GET, "/metrics") => {
                            Ok::<_, hyper::Error>(Response::new(dump().into()))
                        }
                        (Method::PUT, path) if path.starts_with("/log-level") => {
                            Self::change_log_level(req).await
                        }
                        _ => Ok(StatusServer::err_response(
                            StatusCode::NOT_FOUND,
                            "path not found",
                        )),
                    }
                }))
            }
        }));

        let rx = self.rx.take().unwrap();
        let graceful = server
            .with_graceful_shutdown(async move {
                let _ = rx.await;
            })
            .map_err(|e| warn!("Status server error: {:?}", e));
        self.thread_pool.spawn(graceful);
    }

    pub fn start(&mut self, status_addr: String) -> Result<()> {
        let addr = SocketAddr::from_str(&status_addr).unwrap();

        let incoming = {
            let _enter = self.thread_pool.enter();
            AddrIncoming::bind(&addr)
        }
            .unwrap();
        self.addr = Some(incoming.local_addr());

        let server = Server::builder(incoming);
        self.start_serve(server);
        Ok(())
    }

    fn err_response<T>(status_code: StatusCode, message: T) -> Response<Body>
        where
            T: Into<Body>,
    {
        Response::builder()
            .status(status_code)
            .body(message.into())
            .unwrap()
    }
}