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
use crate::connection::postgres_connection_wrapper::PostgresConnectionWrapper;
use crossbeam::channel as crossbeam_channel;
use std::error::Error;
use std::ops::DerefMut;
use std::process;
use std::thread::{self, sleep, JoinHandle};
use std::time::Duration;
use thiserror::Error;
use tracing::{debug, error, info, warn};

static MAX_RETRY: i32 = 3;

pub trait Persistable: Send {
    fn get_insert_key(&self) -> String;
    fn get_insert_value(&self) -> String;
    fn generate_batch_insert_query(&self) -> String;
    fn get_indicator(&self) -> String;
    fn clone_box(&self) -> Box<dyn Persistable>;
}

impl Clone for Box<dyn Persistable> {
    fn clone(&self) -> Self {
        self.clone_box()
    }
}

pub struct PersistThread {
    jh: JoinHandle<()>,
    thread_name: String,
}

impl PersistThread {
    pub fn start_batch_persist(
        thread_name: &str,
        receiver: crossbeam_channel::Receiver<Box<dyn Persistable>>,
    ) -> Self {
        let thread_name = format!("{}_persist_thread", thread_name);

        let thread_name_clone = thread_name.clone();

        let jh = thread::Builder::new()
            .name(format!("{}_persist_thread", &thread_name))
            .spawn(move || PersistThread::batch_persist(&thread_name_clone, receiver))
            .expect("Failed to create finished_workflow_persist_thread");

        PersistThread { jh, thread_name }
    }

    // batch persist Persistable object with same type to Postgres DB
    pub fn batch_persist(
        thread_name: &str,
        receiver: crossbeam_channel::Receiver<Box<dyn Persistable>>,
    ) {
        let mut pg_conn_wrapper = PostgresConnectionWrapper::default();
        pg_conn_wrapper.add_rw_client();

        while let Ok(msg) = receiver.recv() {
            let mut msg_buffer = vec![msg];

            while let Ok(msg) = receiver.try_recv() {
                msg_buffer.push(msg);
                if msg_buffer.len() > 100 {
                    break;
                }
            }

            let begin_info = msg_buffer[0].as_ref().get_indicator();
            let end_info = msg_buffer[msg_buffer.len() - 1].as_ref().get_indicator();

            let queries: Vec<String> = msg_buffer
                .iter()
                .map(|msg| msg.generate_batch_insert_query())
                .collect();

            let queries = queries.join(";");
            debug!("queries: {}", queries);

            if let Err(e) = do_transaction(
                &mut pg_conn_wrapper,
                &queries,
                thread_name,
                (&begin_info, &end_info),
            ) {
                error!("PG transaction failed. Error: {}", e);
                process::exit(1);
            }
        }
    }

    pub fn close(self) {
        info!("Shutting {}", &self.thread_name);
        let _ = self.jh.join();
        info!("Finished shutdown {}", &self.thread_name);
    }
}

fn do_transaction(
    connection_wrapper: &mut PostgresConnectionWrapper,
    queries: &str,
    thread_name: &str,
    debug_info: (&str, &str),
) -> Result<(), Box<dyn Error>> {
    for retry in 0..MAX_RETRY {
        if retry != 0 {
            sleep(Duration::from_secs(5));
        }

        let mut connection = connection_wrapper.fetch_rw_connection()?;
        let client = connection.deref_mut();

        let mut transaction = match client.transaction() {
            Ok(tx) => tx,
            Err(e) => {
                warn!("Failed to create PG transaction: {}", e);
                continue;
            }
        };

        if let Err(e) = transaction.batch_execute(queries) {
            warn!("Failed to execute batch query in PG transaction: {}", e);
            continue;
        }

        if let Err(e) = transaction.commit() {
            warn!("Failed to commit PG transaction: {}", e);
            continue;
        }

        info!(
            "{} thread finished persist  from {} to {}",
            thread_name, debug_info.0, debug_info.1
        );

        return Ok(());
    }

    Err(Box::new(RetryErr {
        message: "PG transaction".to_string(),
        retries: MAX_RETRY as usize,
    }))
}

#[derive(Error, Debug)]
#[error("{message:} failed after retrying ({retries:}) times.")]
pub struct RetryErr {
    message: String,
    retries: usize,
}
