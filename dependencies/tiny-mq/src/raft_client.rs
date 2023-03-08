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

use crossbeam::queue::ArrayQueue;

use futures::task::{Context, Poll, Waker};

use futures::channel::oneshot;
use futures::Sink;
use grpcio::{
    ChannelBuilder, ClientCStreamReceiver, ClientCStreamSender, Environment, RpcStatusCode,
    WriteFlags,
};
use kvproto::raft_serverpb::{Done, RaftMessage};
use kvproto::tikvpb::TikvClient;
use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use futures_util::compat::Future01CompatExt;
use std::ffi::CString;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{Duration, Instant};
use tikv_util::lru::LruCache;
use tikv_util::timer::GLOBAL_TIMER_HANDLE;
use tikv_util::{debug, error, error_unknown, info, thd_name, trace};
use yatp::task::future::TaskCell;
use yatp::ThreadPool;

static CONN_ID: AtomicI32 = AtomicI32::new(0);

/// A quick queue for sending raft messages.
struct Queue {
    buf: ArrayQueue<RaftMessage>,
    waker: Mutex<Option<Waker>>,
}

impl Queue {
    /// Creates a Queue that can store at lease `cap` messages.
    fn with_capacity(cap: usize) -> Queue {
        Queue {
            buf: ArrayQueue::new(cap),
            waker: Mutex::new(None),
        }
    }

    /// Pushes message into the tail of the Queue.
    ///
    /// You are supposed to call `notify` to make sure the message will be sent
    /// finally.
    ///
    /// True when the message is pushed into queue otherwise false.
    fn push(&self, msg: RaftMessage) {
        // todo(glengeng): tikv handles the queue full case, but i removed that code during porting due to limit of time.
        trace!("Queue.push() is called, msg: {:?}", msg);
        self.buf.push(msg).unwrap();
    }

    /// Wakes up consumer to retrive message.
    fn notify(&self) {
        if !self.buf.is_empty() {
            let t = self.waker.lock().unwrap().take();
            if let Some(t) = t {
                trace!("Queue.notify() is called.");
                t.wake();
            }
        }
    }

    /// Gets the buffer len.
    #[inline]
    fn len(&self) -> usize {
        self.buf.len()
    }

    /// Gets message from the head of the queue.
    fn try_pop(&self) -> Option<RaftMessage> {
        self.buf.pop()
    }

    /// Same as `try_pop` but register interest on readiness when `None` is returned.
    ///
    /// The method should be called in polling context. If the queue is empty,
    /// it will register current polling task for notifications.
    #[inline]
    fn pop(&self, ctx: &Context) -> Option<RaftMessage> {
        self.buf.pop().or_else(|| {
            {
                let mut waker = self.waker.lock().unwrap();
                *waker = Some(ctx.waker().clone());
            }
            self.buf.pop()
        })
    }
}

trait Buffer {
    type OutputMessage;

    /// Tests if it is full.
    ///
    /// A full buffer should be flushed successfully before calling `push`.
    fn full(&self) -> bool;
    /// Pushes the message into buffer.
    fn push(&mut self, msg: RaftMessage);
    /// Checks if the batch is empty.
    fn empty(&self) -> bool;
    /// Flushes the message to grpc.
    ///
    /// `sender` should be able to accept messages.
    fn flush(
        &mut self,
        sender: &mut ClientCStreamSender<Self::OutputMessage>,
    ) -> grpcio::Result<()>;
}

/// A buffer for non-batch RaftMessage.
struct MessageBuffer {
    batch: VecDeque<RaftMessage>,
}

impl MessageBuffer {
    fn new() -> MessageBuffer {
        MessageBuffer {
            batch: VecDeque::with_capacity(2),
        }
    }
}

impl Buffer for MessageBuffer {
    type OutputMessage = RaftMessage;

    #[inline]
    fn full(&self) -> bool {
        self.batch.len() >= 2
    }

    #[inline]
    fn push(&mut self, msg: RaftMessage) {
        self.batch.push_back(msg);
    }

    #[inline]
    fn empty(&self) -> bool {
        self.batch.is_empty()
    }

    #[inline]
    fn flush(&mut self, sender: &mut ClientCStreamSender<RaftMessage>) -> grpcio::Result<()> {
        if let Some(msg) = self.batch.pop_front() {
            trace!("Buffer.flush() is called, msg: {:?}", msg);
            Pin::new(sender).start_send((
                msg,
                WriteFlags::default().buffer_hint(!self.batch.is_empty()),
            ))
        } else {
            Ok(())
        }
    }
}

fn grpc_error_is_unimplemented(e: &grpcio::Error) -> bool {
    if let grpcio::Error::RpcFailure(ref status) = e {
        status.code() == RpcStatusCode::UNIMPLEMENTED
    } else {
        false
    }
}

/// Struct tracks the lifetime of a `raft` or `batch_raft` RPC.
struct RaftCall<M, B> {
    sender: ClientCStreamSender<M>,
    receiver: ClientCStreamReceiver<Done>,
    queue: Arc<Queue>,
    buffer: B,
    lifetime: Option<oneshot::Sender<()>>,
    store_id: u64,
    addr: String,
}

impl<M, B> RaftCall<M, B>
where
    B: Buffer<OutputMessage = M>,
{
    fn fill_msg(&mut self, ctx: &Context) {
        trace!("RaftCall.fill_msg() is entered.");
        while !self.buffer.full() {
            let msg = match self.queue.pop(ctx) {
                Some(msg) => msg,
                None => return,
            };
            trace!("RaftCall.fill_msg() is called, msg: {:?}", msg);
            self.buffer.push(msg);
        }
    }

    fn clean_up(&mut self, sink_err: &Option<grpcio::Error>, recv_err: &Option<grpcio::Error>) {
        error!("connection aborted"; "store_id" => self.store_id, "sink_error" => ?sink_err, "receiver_err" => ?recv_err, "addr" => %self.addr);

        if let Some(tx) = self.lifetime.take() {
            let should_fallback = [sink_err, recv_err]
                .iter()
                .any(|e| e.as_ref().map_or(false, grpc_error_is_unimplemented));
            if should_fallback {
                // Asks backend to fallback.
                let _ = tx.send(());
                return;
            }
        }
    }
}

impl<M, B> Future for RaftCall<M, B>
where
    B: Buffer<OutputMessage = M> + Unpin,
{
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, ctx: &mut Context) -> Poll<()> {
        let s = &mut *self;
        trace!("RaftCall.poll() is entered.");
        loop {
            s.fill_msg(ctx);
            if !s.buffer.empty() {
                let mut res = Pin::new(&mut s.sender).poll_ready(ctx);
                if let Poll::Ready(Ok(())) = res {
                    res = Poll::Ready(s.buffer.flush(&mut s.sender));
                }
                match res {
                    Poll::Ready(Ok(())) => continue,
                    Poll::Pending => {
                        trace!("RaftCall.poll() is left from sender.poll_ready()");
                        return Poll::Pending;
                    }
                    Poll::Ready(Err(e)) => {
                        let re = match Pin::new(&mut s.receiver).poll(ctx) {
                            Poll::Ready(Err(e)) => Some(e),
                            _ => None,
                        };
                        s.clean_up(&Some(e), &re);
                        return Poll::Ready(());
                    }
                }
            }

            if let Poll::Ready(Err(e)) = Pin::new(&mut s.sender).poll_flush(ctx) {
                let re = match Pin::new(&mut s.receiver).poll(ctx) {
                    Poll::Ready(Err(e)) => Some(e),
                    _ => None,
                };
                s.clean_up(&Some(e), &re);
                return Poll::Ready(());
            }

            match Pin::new(&mut s.receiver).poll(ctx) {
                Poll::Pending => {
                    trace!("RaftCall.poll() is left from receiver.poll()");
                    return Poll::Pending;
                }
                Poll::Ready(Ok(_)) => {
                    info!("connection close"; "store_id" => s.store_id, "addr" => %s.addr);
                    return Poll::Ready(());
                }
                Poll::Ready(Err(e)) => {
                    s.clean_up(&None, &Some(e));
                    return Poll::Ready(());
                }
            }
        }
    }
}

#[derive(Default, Clone)]
pub struct AddressMap {
    addrs: Arc<Mutex<HashMap<u64, String>>>,
}

impl AddressMap {
    pub fn get(&self, store_id: u64) -> Option<String> {
        let addrs = self.addrs.lock().unwrap();
        addrs.get(&store_id).cloned()
    }

    pub fn insert(&mut self, store_id: u64, addr: String) {
        self.addrs.lock().unwrap().insert(store_id, addr);
    }

    pub fn resolve(&self, store_id: u64) -> String {
        let addr = self.get(store_id);
        match addr {
            Some(addr) => return addr,
            None => panic!("unable to find address for store {}", store_id),
        }
    }
}

#[derive(Clone)]
pub struct ConnectionBuilder {
    env: Arc<Environment>,
    resolver: AddressMap,
}

impl ConnectionBuilder {
    pub fn new(env: Arc<Environment>, resolver: AddressMap) -> ConnectionBuilder {
        ConnectionBuilder { env, resolver }
    }
}

/// StreamBackEnd watches lifetime of a connection and handles reconnecting,
/// spawn new RPC.
struct StreamBackEnd {
    store_id: u64,
    queue: Arc<Queue>,
    builder: ConnectionBuilder,
}

impl StreamBackEnd {
    fn resolve(&self) -> String {
        self.builder.resolver.resolve(self.store_id)
    }

    fn clear_pending_message(&self, reason: &str) {
        let len = self.queue.len();
        for _ in 0..len {
            self.queue.try_pop().unwrap();
        }
    }

    fn connect(&self, addr: &str) -> TikvClient {
        info!("server: new connection with tikv endpoint"; "addr" => addr, "store_id" => self.store_id);

        let cb = ChannelBuilder::new(self.builder.env.clone())
            .stream_initial_window_size(2 * 1024 * 1024 as i32)
            .max_send_message_len(10 * 1024 * 1024)
            .keepalive_time(Duration::from_secs(10))
            .keepalive_timeout(Duration::from_secs(3))
            // .default_compression_algorithm(CompressionAlgorithms::GRPC_COMPRESS_NONE)
            // hack: so it's different args, grpc will always create a new connection.
            .raw_cfg_int(
                CString::new("random id").unwrap(),
                CONN_ID.fetch_add(1, Ordering::SeqCst),
            );
        let channel = cb.connect(addr);
        TikvClient::new(channel)
    }

    fn call(&self, client: &TikvClient, addr: String) -> oneshot::Receiver<()> {
        let (sink, stream) = client.raft().unwrap();
        let (tx, rx) = oneshot::channel();
        let call = RaftCall {
            sender: sink,
            receiver: stream,
            queue: self.queue.clone(),
            buffer: MessageBuffer::new(),
            lifetime: Some(tx),
            store_id: self.store_id,
            addr,
        };
        client.spawn(call);
        rx
    }
}

async fn maybe_backoff() {
    let timeout = Duration::from_secs(1);
    let now = Instant::now();

    if let Err(e) = GLOBAL_TIMER_HANDLE.delay(now + timeout).compat().await {
        error!("something is wrong with GLOBAL_TIMER_HANDLE");
    }
}

/// A future that drives the life cycle of a connection.
///
/// The general progress of connection is:
///
/// 1. resolve address
/// 2. connect
/// 3. make batch call
/// 4. fallback to legacy API if incompatible
///
/// Every failure during the process should trigger retry automatically.
async fn start(back_end: StreamBackEnd, conn_id: usize, pool: Arc<Mutex<ConnectionPool>>) {
    let mut last_wake_time = Instant::now();
    let mut retry_times = 0;
    loop {
        maybe_backoff().await;
        retry_times += 1;
        let addr = back_end.resolve();

        let client = back_end.connect(&addr);

        // If the call is setup successfully, it will never finish. Returning `Ok(())` means the
        // batch_call is not supported, we are probably connect to an old version of TiKV. So we
        // need to fallback to use legacy API.
        let f = back_end.call(&client, addr.clone());

        match f.await {
            Ok(()) => {
                error!("connection fail"; "store_id" => back_end.store_id, "addr" => addr, "err" => "require fallback even with legacy API");
            }
            Err(_) => {
                error!("connection abort"; "store_id" => back_end.store_id, "addr" => addr);
                if retry_times > 1 {
                    // Clears pending messages to avoid consuming high memory when one node is shutdown.
                    back_end.clear_pending_message("unreachable");
                }
            }
        }
    }
}

/// A global connection pool.
///
/// All valid connections should be stored as a record. Once it's removed
/// from the struct, all cache clone should also remove it at some time.
#[derive(Default)]
struct ConnectionPool {
    connections: HashMap<(u64, usize), Arc<Queue>>,
}

/// Queue in cache.
struct CachedQueue {
    queue: Arc<Queue>,
    /// If a msg is enqueued, but the queue has not been notified for polling,
    /// it will be marked to true. And all dirty queues are expected to be
    /// notified during flushing.
    dirty: bool,
    // /// Mark if the connection is full.
    // full: bool,
}

/// A raft client that can manages connections correctly.
///
/// A correct usage of raft client is:
///
/// ```text
/// for m in msgs {
///     if !raft_client.send(m) {
///         // handle error.
///     }
/// }
/// raft_client.flush();
/// ```
pub struct RaftClient {
    pool: Arc<Mutex<ConnectionPool>>,
    cache: LruCache<(u64, usize), CachedQueue>,
    need_flush: Vec<(u64, usize)>,
    // full_stores: Vec<(u64, usize)>,
    future_pool: Arc<ThreadPool<TaskCell>>,
    builder: ConnectionBuilder,
    // builder: ConnectionBuilder<S, R>,
    // engine: PhantomData<E>,
    // last_hash: (u64, u64),
}

impl RaftClient {
    pub fn new(builder: ConnectionBuilder) -> RaftClient {
        let future_pool = Arc::new(
            yatp::Builder::new(thd_name!("raft-stream"))
                .max_thread_count(1)
                .build_future_pool(),
        );
        RaftClient {
            pool: Arc::default(),
            cache: LruCache::with_capacity_and_sample(0, 7),
            need_flush: vec![],
            //            full_stores: vec![],
            future_pool,
            builder,
            //            engine: PhantomData::<E>,
            //            last_hash: (0, 0),
        }
    }

    /// Loads connection from pool.
    ///
    /// Creates it if it doesn't exist. `false` is returned if such connection
    /// can't be established.
    fn load_stream(&mut self, store_id: u64, conn_id: usize) -> bool {
        let (s, pool_len) = {
            let mut pool = self.pool.lock().unwrap();
            // if pool.tombstone_stores.contains(&store_id) {
            //     let pool_len = pool.connections.len();
            //     drop(pool);
            //     self.cache.resize(pool_len);
            //     return false;
            // }
            let conn = pool
                .connections
                .entry((store_id, conn_id))
                .or_insert_with(|| {
                    let queue = Arc::new(Queue::with_capacity(
                        // self.builder.cfg.raft_client_queue_size,
                        819200,
                    ));
                    let back_end = StreamBackEnd {
                        store_id,
                        queue: queue.clone(),
                        builder: self.builder.clone(),
                        // engine: PhantomData::<E>,
                    };
                    self.future_pool
                        .spawn(start(back_end, conn_id, self.pool.clone()));
                    queue
                })
                .clone();
            (conn, pool.connections.len())
        };
        self.cache.resize(pool_len);
        self.cache.insert(
            (store_id, conn_id),
            CachedQueue {
                queue: s,
                dirty: false,
                // full: false,
            },
        );
        true
    }

    /// Sends a message.
    ///
    /// If the message fails to be sent, false is returned. Returning true means the message is
    /// enqueued to buffer. Caller is expected to call `flush` to ensure all buffered messages
    /// are sent out.
    // pub fn send(&mut self, msg: RaftMessage) -> result::Result<(), DiscardReason> {
    pub fn send(&mut self, msg: RaftMessage) {
        let store_id = msg.get_to_peer().store_id;
        let conn_id = 0;
        loop {
            if let Some(s) = self.cache.get_mut(&(store_id, conn_id)) {
                s.queue.push(msg);
                if !s.dirty {
                    s.dirty = true;
                    self.need_flush.push((store_id, conn_id));
                }
                return;
            }
            if !self.load_stream(store_id, conn_id) {
                error!("failed to call load_stream");
            }
        }
    }

    pub fn need_flush(&self) -> bool {
        !self.need_flush.is_empty()
    }

    /// Flushes all buffered messages.
    pub fn flush(&mut self) {
        // self.flush_full_metrics();
        if self.need_flush.is_empty() {
            return;
        }
        for id in &self.need_flush {
            if let Some(s) = self.cache.get_mut(id) {
                if s.dirty {
                    s.dirty = false;
                    s.queue.notify();
                }
                continue;
            }
            let l = self.pool.lock().unwrap();
            if let Some(q) = l.connections.get(id) {
                q.notify();
            }
        }
        self.need_flush.clear();
    }
}

impl Clone for RaftClient {
    fn clone(&self) -> Self {
        RaftClient {
            pool: self.pool.clone(),
            cache: LruCache::with_capacity_and_sample(0, 7),
            need_flush: vec![],
            // full_stores: vec![],
            future_pool: self.future_pool.clone(),
            builder: self.builder.clone(),
            // engine: PhantomData::<E>,
            // last_hash: (0, 0),
        }
    }
}
