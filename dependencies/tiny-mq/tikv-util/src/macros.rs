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

/// Makes a thread name with an additional tag inherited from the current thread.
#[macro_export]
macro_rules! thd_name {
    ($name:expr) => {{
        $crate::get_tag_from_thread_name()
            .map(|tag| format!("{}::{}", $name, tag))
            .unwrap_or_else(|| $name.to_owned())
    }};
}

/// A shortcut to box an error.
#[macro_export]
macro_rules! box_err {
    ($e:expr) => ({
        use std::error::Error;
        let e: Box<dyn Error + Sync + Send> = format!("[{}:{}]: {}", file!(), line!(),  $e).into();
        e.into()
    });
    ($f:tt, $($arg:expr),+) => ({
        box_err!(format!($f, $($arg),+))
    });
}

/// Boxes error first, and then does the same thing as `try!`.
#[macro_export]
macro_rules! box_try {
    ($expr:expr) => {{
        match $expr {
            Ok(r) => r,
            Err(e) => return Err($crate::box_err!(e)),
        }
    }};
}

/// Logs slow operations by `warn!`.
/// The final log level depends on the given `cost` and `slow_log_threshold`
#[macro_export]
macro_rules! slow_log {
    (T $t:expr, $($arg:tt)*) => {{
        if $t.is_slow() {
            warn!(#"slow_log_by_timer", $($arg)*; "takes" => $crate::logger::LogCost($crate::time::duration_to_ms($t.elapsed())));
        }
    }};
    ($n:expr, $($arg:tt)*) => {{
        warn!(#"slow_log", $($arg)*; "takes" => $crate::logger::LogCost($crate::time::duration_to_ms($n)));
    }}

}

/// A safe panic macro that prevents double panic.
///
/// You probably want to use this macro instead of `panic!` in a `drop` method.
/// It checks whether the current thread is unwinding because of panic. If it is,
/// log an error message instead of causing double panic.
#[macro_export]
macro_rules! safe_panic {
    () => ({
        safe_panic!("explicit panic")
    });
    ($msg:expr) => ({
        if std::thread::panicking() {
            error!(concat!($msg, ", double panic prevented"))
        } else {
            panic!($msg)
        }
    });
    ($fmt:expr, $($args:tt)+) => ({
        if std::thread::panicking() {
            error!(concat!($fmt, ", double panic prevented"), $($args)+)
        } else {
            panic!($fmt, $($args)+)
        }
    });
}