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

/// Provides util functions to manage share properties across threads.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[derive(Default)]
struct GroupPropertiesInner {
    shutdown: AtomicBool,
}

#[derive(Default, Clone)]
pub struct GroupProperties {
    inner: Arc<GroupPropertiesInner>,
}

impl GroupProperties {
    #[inline]
    pub fn mark_shutdown(&self) {
        self.inner.shutdown.store(true, Ordering::SeqCst);
    }
}

thread_local! {
    static PROPERTIES: RefCell<Option<GroupProperties>> = RefCell::new(None);
}

pub fn current_properties() -> Option<GroupProperties> {
    PROPERTIES.with(|p| p.borrow().clone())
}

pub fn set_properties(props: Option<GroupProperties>) {
    PROPERTIES.with(move |p| {
        p.replace(props);
    })
}

/// Checks if the system is shutdown.
pub fn is_shutdown(ensure_set: bool) -> bool {
    PROPERTIES.with(|p| {
        if let Some(props) = &*p.borrow() {
            props.inner.shutdown.load(Ordering::SeqCst)
        } else if ensure_set {
            safe_panic!("group properties is not set");
            false
        } else {
            false
        }
    })
}

pub fn mark_shutdown() {
    PROPERTIES.with(|p| {
        if let Some(props) = &mut *p.borrow_mut() {
            props.mark_shutdown();
        }
    })
}
