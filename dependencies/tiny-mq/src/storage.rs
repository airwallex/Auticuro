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

use crate::errors::{Error, Result};
use crate::raftkv::{CmdRes, RaftKv};
use crate::rocks_engine::{CfName, ALL_CFS, CF_DEFAULT};
use futures::prelude::*;
use kvproto::kvrpcpb::{Context, MessageQueueRequest, MessageQueueResponse};
use std::sync::{atomic, Arc};
use tikv_util::{box_err, info, trace};

pub type Callback<T> = Box<dyn FnOnce(Result<T>) + Send>;

#[derive(Debug, Clone)]
pub enum Modify {
    MessageQueue(MessageQueueRequest),
}

#[derive(Default)]
pub struct WriteData {
    pub modifies: Vec<Modify>,
}

impl WriteData {
    pub fn new(modifies: Vec<Modify>) -> Self {
        Self { modifies }
    }

    pub fn from_modifies(modifies: Vec<Modify>) -> Self {
        Self::new(modifies)
    }
}

// pub struct Storage<E: Engine, L: LockManager> {
pub struct Storage {
    // TODO: Too many Arcs, would be slow when clone.
    engine: RaftKv,

    /// How many strong references. Thread pool and workers will be stopped
    /// once there are no more references.
    // TODO: This should be implemented in thread pool and worker.
    refs: Arc<atomic::AtomicUsize>,
}

impl Clone for Storage {
    #[inline]
    fn clone(&self) -> Self {
        let refs = self.refs.fetch_add(1, atomic::Ordering::SeqCst);

        trace!(
            "Storage referenced"; "original_ref" => refs
        );

        Self {
            engine: self.engine.clone(),
            refs: self.refs.clone(),
        }
    }
}

impl Drop for Storage {
    #[inline]
    fn drop(&mut self) {
        let refs = self.refs.fetch_sub(1, atomic::Ordering::SeqCst);

        trace!(
            "Storage de-referenced"; "original_ref" => refs
        );

        if refs != 1 {
            return;
        }

        info!("Storage stopped.");
    }
}

impl Storage {
    /// Create a `Storage` from given engine.
    pub fn from_engine(engine: RaftKv) -> Result<Self> {
        info!("Storage started.");

        Ok(Storage {
            engine,
            refs: Arc::new(atomic::AtomicUsize::new(1)),
        })
    }

    /// Get the underlying `Engine` of the `Storage`.
    pub fn get_engine(&self) -> RaftKv {
        self.engine.clone()
    }

    pub fn raw_message_queue(
        &self,
        message_queue_req: MessageQueueRequest,
        callback: Callback<MessageQueueResponse>,
    ) -> Result<()> {
        let ctx = message_queue_req.get_context().clone();
        let m = Modify::MessageQueue(message_queue_req);
        let batch = WriteData::from_modifies(vec![m]);

        self.engine
            .exec_write_requests(
                &ctx,
                batch,
                Box::new(move |(_cb_ctx, res)| match res {
                    Ok(CmdRes::Resp(mut resps)) => {
                        assert_eq!(resps.len(), 1);
                        callback(Ok(resps.pop().unwrap().take_message_queue()))
                    }
                    Err(e) => callback(Err(e)),
                }),
                None,
                None,
            )
            .unwrap();

        Ok(())
    }
}
