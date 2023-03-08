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

use std::thread;
use std::ops::{DerefMut, Deref};

#[macro_use]
pub mod log;
pub mod lru;
#[macro_use]
pub mod macros;
pub mod timer;
pub mod time;
pub mod future;
pub mod keybuilder;
pub mod logger;
pub mod worker;
pub mod thread_group;
pub mod yatp_pool;

pub trait AssertClone: Clone {}

pub trait AssertCopy: Copy {}

pub trait AssertSend: Send {}

pub trait AssertSync: Sync {}

pub fn get_tag_from_thread_name() -> Option<String> {
    thread::current()
        .name()
        .and_then(|name| name.split("::").skip(1).last())
        .map(From::from)
}

/// Represents a value of one of two possible types (a more generic Result.)
#[derive(Debug, Clone)]
pub enum Either<L, R> {
    Left(L),
    Right(R),
}

impl<L, R> Either<L, R> {
    #[inline]
    pub fn as_ref(&self) -> Either<&L, &R> {
        match *self {
            Either::Left(ref l) => Either::Left(l),
            Either::Right(ref r) => Either::Right(r),
        }
    }

    #[inline]
    pub fn as_mut(&mut self) -> Either<&mut L, &mut R> {
        match *self {
            Either::Left(ref mut l) => Either::Left(l),
            Either::Right(ref mut r) => Either::Right(r),
        }
    }

    #[inline]
    pub fn left(self) -> Option<L> {
        match self {
            Either::Left(l) => Some(l),
            _ => None,
        }
    }

    #[inline]
    pub fn right(self) -> Option<R> {
        match self {
            Either::Right(r) => Some(r),
            _ => None,
        }
    }
}

pub struct MustConsumeVec<T> {
    tag: &'static str,
    v: Vec<T>,
}

impl<T> MustConsumeVec<T> {
    #[inline]
    pub fn new(tag: &'static str) -> MustConsumeVec<T> {
        MustConsumeVec::with_capacity(tag, 0)
    }

    #[inline]
    pub fn with_capacity(tag: &'static str, cap: usize) -> MustConsumeVec<T> {
        MustConsumeVec {
            tag,
            v: Vec::with_capacity(cap),
        }
    }

    pub fn take(&mut self) -> Self {
        MustConsumeVec {
            tag: self.tag,
            v: std::mem::take(&mut self.v),
        }
    }
}

impl<T> Deref for MustConsumeVec<T> {
    type Target = Vec<T>;

    fn deref(&self) -> &Vec<T> {
        &self.v
    }
}

impl<T> DerefMut for MustConsumeVec<T> {
    fn deref_mut(&mut self) -> &mut Vec<T> {
        &mut self.v
    }
}

impl<T> Drop for MustConsumeVec<T> {
    fn drop(&mut self) {
        if !self.is_empty() {
            safe_panic!("resource leak detected: {}.", self.tag);
        }
    }
}
