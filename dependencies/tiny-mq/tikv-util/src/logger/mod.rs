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

// mod file_log;
mod formatter;

// use std::env;
use std::fmt;
// use std::io::{self, BufWriter};
use std::io;
// use std::path::{Path, PathBuf};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
// use std::thread;

use log::{self, SetLoggerError};
// use slog::{self, slog_o, Drain, FnValue, Key, OwnedKVList, PushFnValue, Record, KV};
use slog::{self, slog_o, Drain, Key, OwnedKVList, Record, KV};
use slog_async::{Async, AsyncGuard, OverflowStrategy};
use slog_term::{Decorator, PlainDecorator, RecordDecorator};

// use self::file_log::{RotateBySize, RotateByTime, RotatingFileLogger, RotatingFileLoggerBuilder};
// use crate::config::{ReadableDuration, ReadableSize};

pub use slog::{FilterFn, Level};

// // The suffix appended to the end of rotated log files by datetime log rotator
// // Warning: Diagnostics service parses log files by file name format.
// //          Remember to update the corresponding code when suffix layout is changed.
// pub const DATETIME_ROTATE_SUFFIX: &str = "%Y-%m-%d-%H:%M:%S%.f";

// Default is 128.
// Extended since blocking is set, and we don't want to block very often.
const SLOG_CHANNEL_SIZE: usize = 10240;
// Default is DropAndReport.
// It is not desirable to have dropped logs in our use case.
const SLOG_CHANNEL_OVERFLOW_STRATEGY: OverflowStrategy = OverflowStrategy::Drop;
const TIMESTAMP_FORMAT: &str = "%Y/%m/%d %H:%M:%S%.3f %:z";

static LOG_LEVEL: AtomicUsize = AtomicUsize::new(usize::max_value());

pub fn init_log<D>(
    drain: D,
    level: Level,
    use_async: bool,
    init_stdlog: bool,
    // mut disabled_targets: Vec<String>,
    // slow_threshold: u64,
) -> Result<(), SetLoggerError>
    where
        D: Drain + Send + 'static,
        <D as Drain>::Err: std::fmt::Display,
{
    // Set the initial log level used by the Drains
    LOG_LEVEL.store(level.as_usize(), Ordering::Relaxed);

    // // Only for debug purpose, so use environment instead of configuration file.
    // if let Ok(extra_modules) = env::var("TIKV_DISABLE_LOG_TARGETS") {
    //     disabled_targets.extend(extra_modules.split(',').map(ToOwned::to_owned));
    // }

    let filter = move |_record: &Record| {
        // if !disabled_targets.is_empty() {
        //     // The format of the returned value from module() would like this:
        //     // ```
        //     //  raftstore::store::fsm::store
        //     //  tikv_util
        //     //  tikv_uti`l::config::check_data_dir
        //     //  raft::raft
        //     //  grpcio::log_util
        //     //  ...
        //     // ```
        //     // Here get the highest level module name to check.
        //     let module = record.module().splitn(2, "::").next().unwrap();
        //     disabled_targets.iter().all(|target| target != module)
        // } else {
            true
        // }
    };

    let (logger, guard) = if use_async {
        let (async_log, guard) = Async::new(LogAndFuse(drain))
            .chan_size(SLOG_CHANNEL_SIZE)
            .overflow_strategy(SLOG_CHANNEL_OVERFLOW_STRATEGY)
            .thread_name(thd_name!("slogger"))
            .build_with_guard();
        let drain = async_log.filter_level(slog::Level::Trace).fuse();
        // let drain = SlowLogFilter {
        //     threshold: slow_threshold,
        //     inner: drain,
        // };
        let filtered = drain.filter(filter).fuse();

        (slog::Logger::root(filtered, slog_o!()), Some(guard))
    } else {
        let drain = LogAndFuse(Mutex::new(drain).filter_level(slog::Level::Trace));
        // let drain = SlowLogFilter {
        //     threshold: slow_threshold,
        //     inner: drain,
        // };
        let filtered = drain.filter(filter).fuse();
        (slog::Logger::root(filtered, slog_o!()), None)
    };

    set_global_logger(level, init_stdlog, logger, guard)
}

// All Drains are reference-counted by every Logger that uses them. Async drain
// runs a worker thread and sends a termination (and flushing) message only when
// being dropped. Because of that it's actually quite easy to have a left-over
// reference to a Async drain, when terminating: especially on panics or similar
// unwinding event. Typically it's caused be a leftover reference like Logger in
// thread-local variable, global variable, or a thread that is not being joined
// on. So use AsyncGuard to send a flush and termination message to a Async
// worker thread, and wait for it to finish on the guard's own drop.
lazy_static::lazy_static! {
    pub static ref ASYNC_LOGGER_GUARD: Mutex<Option<AsyncGuard>> = Mutex::new(None);
}

pub fn set_global_logger(
    _level: Level,
    init_stdlog: bool,
    logger: slog::Logger,
    guard: Option<AsyncGuard>,
) -> Result<(), SetLoggerError> {
    slog_global::set_global(logger);
    if init_stdlog {
        // slog_global::redirect_std_log(Some(level))?;
        // slog_global::redirect_std_log(Some(slog::Level::Trace))?;
        // grpcio::redirect_log();
    }
    *ASYNC_LOGGER_GUARD.lock().unwrap() = guard;

    Ok(())
}

/// Constructs a new file writer which outputs log to a file at the specified
/// path. The file writer rotates for the specified timespan.
// pub fn file_writer<N>(
//     path: impl AsRef<Path>,
//     rotation_timespan: ReadableDuration,
//     rotation_size: ReadableSize,
//     rename: N,
// ) -> io::Result<BufWriter<RotatingFileLogger>>
//     where
//         N: 'static + Send + Fn(&Path) -> io::Result<PathBuf>,
// {
//     let logger = BufWriter::new(
//         RotatingFileLoggerBuilder::new(path, rename)
//             .add_rotator(RotateByTime::new(rotation_timespan))
//             .add_rotator(RotateBySize::new(rotation_size))
//             .build()?,
//     );
//     Ok(logger)
// }

/// Constructs a new terminal writer which outputs logs to stderr.
pub fn term_writer() -> io::Stderr {
    io::stderr()
}

/// Formats output logs to "TiDB Log Format".
pub fn text_format<W>(io: W) -> TikvFormat<PlainDecorator<W>>
    where
        W: io::Write,
{
    let decorator = PlainDecorator::new(io);
    TikvFormat::new(decorator)
}

/// Same as text_format, but is adjusted to be closer to vanilla RocksDB logger format.
// pub fn rocks_text_format<W>(io: W) -> RocksFormat<PlainDecorator<W>>
//     where
//         W: io::Write,
// {
//     let decorator = PlainDecorator::new(io);
//     RocksFormat::new(decorator)
// }

/// Formats output logs to JSON format.
// pub fn json_format<W>(io: W) -> slog_json::Json<W>
//     where
//         W: io::Write,
// {
//     slog_json::Json::new(io)
//         .set_newlines(true)
//         .set_flush(true)
//         .add_key_value(slog_o!(
//             "message" => PushFnValue(|record, ser| ser.emit(record.msg())),
//             "caller" => PushFnValue(|record, ser| ser.emit(format_args!(
//                 "{}:{}",
//                 Path::new(record.file())
//                     .file_name()
//                     .and_then(|path| path.to_str())
//                     .unwrap_or("<unknown>"),
//                 record.line(),
//             ))),
//             "level" => FnValue(|record| get_unified_log_level(record.level())),
//             "time" => FnValue(|_| chrono::Local::now().format(TIMESTAMP_FORMAT).to_string()),
//         ))
//         .build()
// }

pub fn get_level_by_string(lv: &str) -> Option<Level> {
    match &*lv.to_owned().to_lowercase() {
        "critical" => Some(Level::Critical),
        "error" => Some(Level::Error),
        // We support `warn` due to legacy.
        "warning" | "warn" => Some(Level::Warning),
        "debug" => Some(Level::Debug),
        "trace" => Some(Level::Trace),
        "info" => Some(Level::Info),
        _ => None,
    }
}

// The `to_string()` function of `slog::Level` produces values like `erro` and `trce` instead of
// the full words. This produces the full word.
pub fn get_string_by_level(lv: Level) -> &'static str {
    match lv {
        Level::Critical => "critical",
        Level::Error => "error",
        Level::Warning => "warning",
        Level::Debug => "debug",
        Level::Trace => "trace",
        Level::Info => "info",
    }
}

// Converts `slog::Level` to unified log level format.
fn get_unified_log_level(lv: Level) -> &'static str {
    match lv {
        Level::Critical => "FATAL",
        Level::Error => "ERROR",
        Level::Warning => "WARN",
        Level::Info => "INFO",
        Level::Debug => "DEBUG",
        Level::Trace => "TRACE",
    }
}

// pub fn convert_slog_level_to_log_level(lv: Level) -> log::Level {
//     match lv {
//         Level::Critical | Level::Error => log::Level::Error,
//         Level::Warning => log::Level::Warn,
//         Level::Debug => log::Level::Debug,
//         Level::Trace => log::Level::Trace,
//         Level::Info => log::Level::Info,
//     }
// }

// pub fn convert_log_level_to_slog_level(lv: log::Level) -> Level {
//     match lv {
//         log::Level::Error => Level::Error,
//         log::Level::Warn => Level::Warning,
//         log::Level::Debug => Level::Debug,
//         log::Level::Trace => Level::Trace,
//         log::Level::Info => Level::Info,
//     }
// }

pub fn get_log_level() -> Option<Level> {
    Level::from_usize(LOG_LEVEL.load(Ordering::Relaxed))
}

pub fn set_log_level(new_level: Level) {
    LOG_LEVEL.store(new_level.as_usize(), Ordering::SeqCst)
}

pub struct TikvFormat<D>
    where
        D: Decorator,
{
    decorator: D,
}

impl<D> TikvFormat<D>
    where
        D: Decorator,
{
    pub fn new(decorator: D) -> Self {
        Self { decorator }
    }
}

impl<D> Drain for TikvFormat<D>
    where
        D: Decorator,
{
    type Ok = ();
    type Err = io::Error;

    fn log(&self, record: &Record<'_>, values: &OwnedKVList) -> Result<Self::Ok, Self::Err> {
        if record.level().as_usize() <= LOG_LEVEL.load(Ordering::Relaxed) {
            self.decorator.with_record(record, values, |decorator| {
                write_log_header(decorator, record)?;
                write_log_msg(decorator, record)?;
                write_log_fields(decorator, record, values)?;

                decorator.start_whitespace()?;
                writeln!(decorator)?;

                decorator.flush()?;

                Ok(())
            })?;
        }

        Ok(())
    }
}

// pub struct RocksFormat<D>
//     where
//         D: Decorator,
// {
//     decorator: D,
// }

// impl<D> RocksFormat<D>
//     where
//         D: Decorator,
// {
//     pub fn new(decorator: D) -> Self {
//         Self { decorator }
//     }
// }

// impl<D> Drain for RocksFormat<D>
//     where
//         D: Decorator,
// {
//     type Ok = ();
//     type Err = io::Error;
//
//     fn log(&self, record: &Record<'_>, values: &OwnedKVList) -> Result<Self::Ok, Self::Err> {
//         self.decorator.with_record(record, values, |decorator| {
//             if !record.tag().ends_with("_header") {
//                 decorator.start_timestamp()?;
//                 write!(
//                     decorator,
//                     "[{}][{}]",
//                     chrono::Local::now().format(TIMESTAMP_FORMAT),
//                     thread::current().id().as_u64(),
//                 )?;
//                 decorator.start_level()?;
//                 write!(decorator, "[{}]", get_unified_log_level(record.level()))?;
//                 decorator.start_whitespace()?;
//                 write!(decorator, " ")?;
//             }
//             decorator.start_msg()?;
//             let msg = format!("{}", record.msg());
//             write!(decorator, "{}", msg)?;
//             if !msg.ends_with('\n') {
//                 writeln!(decorator)?;
//             }
//             decorator.flush()?;
//
//             Ok(())
//         })
//     }
// }

struct LogAndFuse<D>(D);

impl<D> Drain for LogAndFuse<D>
    where
        D: Drain,
        <D as Drain>::Err: std::fmt::Display,
{
    type Ok = ();
    type Err = slog::Never;

    fn log(&self, record: &Record<'_>, values: &OwnedKVList) -> Result<Self::Ok, Self::Err> {
        if record.level().as_usize() <= LOG_LEVEL.load(Ordering::Relaxed) {
            if let Err(e) = self.0.log(record, values) {
                let fatal_drainer = Mutex::new(text_format(term_writer())).ignore_res();
                fatal_drainer.log(record, values).unwrap();
                let fatal_logger = slog::Logger::root(fatal_drainer, slog_o!());
                slog::slog_crit!(
                    fatal_logger,
                    "logger encountered error";
                    "err" => %e,
                );
            }
        }
        Ok(())
    }
}

// Filters logs with operation cost lower than threshold. Otherwise output logs to inner drainer
// struct SlowLogFilter<D> {
//     threshold: u64,
//     inner: D,
// }
//
// impl<D> Drain for SlowLogFilter<D>
//     where
//         D: Drain<Ok = (), Err = slog::Never>,
// {
//     type Ok = ();
//     type Err = slog::Never;
//
//     fn log(&self, record: &Record, values: &OwnedKVList) -> Result<Self::Ok, Self::Err> {
//         if record.tag() == "slow_log" {
//             let mut s = SlowCostSerializer { cost: None };
//             let kv = record.kv();
//             let _ = kv.serialize(record, &mut s);
//             if let Some(cost) = s.cost {
//                 if cost <= self.threshold {
//                     // Filter slow logs which are actually not that slow
//                     return Ok(());
//                 }
//             }
//         }
//         self.inner.log(record, values)
//     }
// }

// struct SlowCostSerializer {
//     // None means input record without key `takes`
//     cost: Option<u64>,
// }
//
// impl slog::ser::Serializer for SlowCostSerializer {
//     fn emit_arguments(&mut self, _key: Key, _val: &fmt::Arguments<'_>) -> slog::Result {
//         Ok(())
//     }
//
//     fn emit_u64(&mut self, key: Key, val: u64) -> slog::Result {
//         if key == "takes" {
//             self.cost = Some(val);
//         }
//         Ok(())
//     }
// }

// /// Special struct for slow log cost serializing
// pub struct LogCost(pub u64);
//
// impl slog::Value for LogCost {
//     fn serialize(
//         &self,
//         _record: &Record,
//         key: Key,
//         serializer: &mut dyn slog::Serializer,
//     ) -> slog::Result {
//         serializer.emit_u64(key, self.0)
//     }
// }

/// Dispatches logs to a normal `Drain` or a slow-log specialized `Drain` by tag
// pub struct LogDispatcher<N: Drain, R: Drain, S: Drain, T: Drain> {
pub struct LogDispatcher<N: Drain> {
    normal: N,
    // rocksdb: R,
    // raftdb: T,
    // slow: Option<S>,
}

// impl<N: Drain, R: Drain, S: Drain, T: Drain> LogDispatcher<N, R, S, T> {
impl<N: Drain> LogDispatcher<N> {
    // pub fn new(normal: N, rocksdb: R, raftdb: T, slow: Option<S>) -> Self {
    pub fn new(normal: N,) -> Self {
        Self {
            normal,
            // rocksdb,
            // raftdb,
            // slow,
        }
    }
}

// impl<N, R, S, T> Drain for LogDispatcher<N, R, S, T>
impl<N> Drain for LogDispatcher<N>
    where
        N: Drain<Ok = (), Err = io::Error>,
        // R: Drain<Ok = (), Err = io::Error>,
        // S: Drain<Ok = (), Err = io::Error>,
        // T: Drain<Ok = (), Err = io::Error>,
{
    type Ok = ();
    type Err = io::Error;

    fn log(&self, record: &Record, values: &OwnedKVList) -> Result<Self::Ok, Self::Err> {
        // let tag = record.tag();
        // if self.slow.is_some() && tag.starts_with("slow_log") {
        //     self.slow.as_ref().unwrap().log(record, values)
        // } else if tag.starts_with("rocksdb_log") {
        //     self.rocksdb.log(record, values)
        // } else if tag.starts_with("raftdb_log") {
        //     self.raftdb.log(record, values)
        // } else {
        //     self.normal.log(record, values)
        // }

        self.normal.log(record, values)
    }
}

/// Writes log header to decorator. See [log-header](https://github.com/tikv/rfcs/blob/master/text/2018-12-19-unified-log-format.md#log-header-section)
fn write_log_header(decorator: &mut dyn RecordDecorator, record: &Record<'_>) -> io::Result<()> {
    decorator.start_timestamp()?;
    write!(
        decorator,
        "[{}]",
        chrono::Local::now().format(TIMESTAMP_FORMAT)
    )?;

    decorator.start_whitespace()?;
    write!(decorator, " ")?;

    decorator.start_level()?;
    write!(decorator, "[{}]", get_unified_log_level(record.level()))?;

    decorator.start_whitespace()?;
    write!(decorator, " ")?;

    // Writes source file info.
    decorator.start_msg()?; // There is no `start_file()` or `start_line()`.
    if let Some(path) = Path::new(record.file())
        .file_name()
        .and_then(|path| path.to_str())
    {
        write!(decorator, "[")?;
        formatter::write_file_name(decorator, path)?;
        write!(decorator, ":{}]", record.line())?
    } else {
        write!(decorator, "[<unknown>]")?
    }

    Ok(())
}

/// Writes log message to decorator. See [log-message](https://github.com/tikv/rfcs/blob/master/text/2018-12-19-unified-log-format.md#log-message-section)
fn write_log_msg(decorator: &mut dyn RecordDecorator, record: &Record<'_>) -> io::Result<()> {
    decorator.start_whitespace()?;
    write!(decorator, " ")?;

    decorator.start_msg()?;
    write!(decorator, "[")?;
    let msg = format!("{}", record.msg());
    formatter::write_escaped_str(decorator, &msg)?;
    write!(decorator, "]")?;

    Ok(())
}

/// Writes log fields to decorator. See [log-fields](https://github.com/tikv/rfcs/blob/master/text/2018-12-19-unified-log-format.md#log-fields-section)
fn write_log_fields(
    decorator: &mut dyn RecordDecorator,
    record: &Record<'_>,
    values: &OwnedKVList,
) -> io::Result<()> {
    let mut serializer = Serializer::new(decorator);

    record.kv().serialize(record, &mut serializer)?;

    values.serialize(record, &mut serializer)?;

    serializer.finish();

    Ok(())
}

struct Serializer<'a> {
    decorator: &'a mut dyn RecordDecorator,
}

impl<'a> Serializer<'a> {
    fn new(decorator: &'a mut dyn RecordDecorator) -> Self {
        Serializer { decorator }
    }

    fn write_whitespace(&mut self) -> io::Result<()> {
        self.decorator.start_whitespace()?;
        write!(self.decorator, " ")?;
        Ok(())
    }

    fn finish(self) {}
}

impl<'a> Drop for Serializer<'a> {
    fn drop(&mut self) {}
}

impl<'a> slog::Serializer for Serializer<'a> {
    fn emit_none(&mut self, key: Key) -> slog::Result {
        self.emit_arguments(key, &format_args!("None"))
    }

    fn emit_arguments(&mut self, key: Key, val: &fmt::Arguments<'_>) -> slog::Result {
        self.write_whitespace()?;

        // Write key
        write!(self.decorator, "[")?;
        self.decorator.start_key()?;
        formatter::write_escaped_str(&mut self.decorator, key as &str)?;

        // Write separator
        self.decorator.start_separator()?;
        write!(self.decorator, "=")?;

        // Write value
        let value = format!("{}", val);
        self.decorator.start_value()?;
        formatter::write_escaped_str(self.decorator, &value)?;
        self.decorator.reset()?;
        write!(self.decorator, "]")?;
        Ok(())
    }
}

pub mod log_level_serde {
    use serde::{
        de::{Error, Unexpected},
        Deserialize, Deserializer, Serialize, Serializer,
    };
    use slog::Level;
    // use tikv_util::logger::{get_level_by_string, get_string_by_level};
    use super::{get_level_by_string, get_string_by_level};

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Level, D::Error>
        where
            D: Deserializer<'de>,
    {
        let string = String::deserialize(deserializer)?;
        get_level_by_string(&string)
            .ok_or_else(|| D::Error::invalid_value(Unexpected::Str(&string), &"a valid log level"))
    }

    #[allow(clippy::trivially_copy_pass_by_ref)]
    pub fn serialize<S>(value: &Level, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
    {
        get_string_by_level(*value).serialize(serializer)
    }
}
