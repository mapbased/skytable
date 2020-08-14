/*
 * Created on Mon Jul 13 2020
 *
 * This file is a part of the source code for the Terrabase database
 * Copyright (c) 2020, Sayan Nandan <ohsayan at outlook dot com>
 *
 * This program is free software: you can redistribute it and/or modify
 * it under the terms of the GNU Affero General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version.
 *
 * This program is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU Affero General Public License for more details.
 *
 * You should have received a copy of the GNU Affero General Public License
 * along with this program. If not, see <https://www.gnu.org/licenses/>.
 *
*/

//! # The core database engine

use crate::diskstore;
use crate::protocol::Query;
use crate::queryengine;
use bytes::Bytes;
use corelib::terrapipe::{ActionType, RespCodes};
use corelib::TResult;
use parking_lot::{RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::collections::{hash_map::Entry, HashMap};
use std::sync::Arc;

/// Results from actions on the Database
pub type ActionResult<T> = Result<T, RespCodes>;

/// This is a thread-safe database handle, which on cloning simply
/// gives another atomic reference to the `Coretable`
#[derive(Debug, Clone)]
pub struct CoreDB {
    shared: Arc<Coretable>,
    terminate: bool,
}

/// The `Coretable` holds all the key-value pairs in a `HashMap`
/// wrapped in a Read/Write lock
#[derive(Debug)]
pub struct Coretable {
    coremap: RwLock<HashMap<String, Data>>,
}

#[derive(Debug)]
pub struct Data {
    blob: Bytes,
}

impl Data {
    pub fn from(val: &String) -> Self {
        Data {
            blob: Bytes::copy_from_slice(val.as_bytes()),
        }
    }
    pub fn from_blob(blob: Bytes) -> Self {
        Data { blob }
    }
    pub fn get_blob(&self) -> &Bytes {
        &self.blob
    }
}

impl CoreDB {
    /// GET a `key`
    pub fn get(&self, key: &str) -> ActionResult<Bytes> {
        if let Some(value) = self.acquire_read().get(key) {
            Ok(value.blob.to_owned())
        } else {
            Err(RespCodes::NotFound)
        }
    }
    /// SET a `key` to `value`
    pub fn set(&self, key: &str, value: &String) -> ActionResult<()> {
        match self.acquire_write().entry(key.to_string()) {
            Entry::Occupied(_) => return Err(RespCodes::OverwriteError),
            Entry::Vacant(e) => {
                let _ = e.insert(Data::from(&value));
                Ok(())
            }
        }
    }
    /// UPDATE a `key` to `value`
    pub fn update(&self, key: &str, value: &String) -> ActionResult<()> {
        match self.acquire_write().entry(key.to_string()) {
            Entry::Occupied(ref mut e) => {
                e.insert(Data::from(&value));
                Ok(())
            }
            Entry::Vacant(_) => Err(RespCodes::NotFound),
        }
    }
    /// DEL a `key`
    pub fn del(&self, key: &str) -> ActionResult<()> {
        if let Some(_) = self.acquire_write().remove(&key.to_owned()) {
            Ok(())
        } else {
            Err(RespCodes::NotFound)
        }
    }

    /// Check if a `key` exists
    pub fn exists(&self, key: &str) -> bool {
        self.acquire_read().contains_key(&key.to_owned())
    }

    #[cfg(Debug)]
    /// Flush the coretable entries when in debug mode
    pub fn print_debug_table(&self) {
        println!("{:#?}", *self.coremap.read().unwrap());
    }

    /// Execute a query that has already been validated by `Connection::read_query`
    pub fn execute_query(&self, df: Query) -> Vec<u8> {
        match df.actiontype {
            ActionType::Simple => queryengine::execute_simple(&self, df.data),
            // TODO(@ohsayan): Pipeline commands haven't been implemented yet
            ActionType::Pipeline => unimplemented!(),
        }
    }

    /// Create a new `CoreDB` instance
    ///
    /// This also checks if a local backup of previously saved data is available.
    /// If it is - it restores the data. Otherwise it creates a new in-memory table
    pub fn new() -> TResult<Self> {
        let coretable = diskstore::get_saved()?;
        if let Some(coretable) = coretable {
            Ok(CoreDB {
                shared: Arc::new(Coretable {
                    coremap: RwLock::new(coretable),
                }),
                terminate: false,
            })
        } else {
            Ok(CoreDB {
                shared: Arc::new(Coretable {
                    coremap: RwLock::new(HashMap::new()),
                }),
                terminate: false,
            })
        }
    }
    /// Acquire a write lock
    fn acquire_write(&self) -> RwLockWriteGuard<'_, HashMap<String, Data>> {
        self.shared.coremap.write()
    }
    /// Acquire a read lock
    fn acquire_read(&self) -> RwLockReadGuard<'_, HashMap<String, Data>> {
        self.shared.coremap.read()
    }
    /// Flush the contents of the in-memory table onto disk
    pub fn flush_db(self) -> TResult<()> {
        let data = &*self.acquire_write();
        diskstore::flush_data(data)?;
        Ok(())
    }

    /// **⚠⚠⚠ This deletes everything stored in the in-memory table**
    pub fn finish_db(self, areyousure: bool, areyouverysure: bool, areyousupersure: bool) {
        if areyousure && areyouverysure && areyousupersure {
            self.acquire_write().clear()
        }
    }
}

impl Drop for CoreDB {
    // This prevents us from killing the database, in the event someone tries
    // to access it
    fn drop(&mut self) {
        if Arc::strong_count(&self.shared) == 1 {
            // Acquire a lock to prevent anyone from writing something
            let coremap = self.shared.coremap.write();
            self.terminate = true;
            drop(coremap);
        }
    }
}
