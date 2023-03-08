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
use r2d2_postgres::postgres::{Config, NoTls};
use r2d2_postgres::r2d2::PooledConnection;
use r2d2_postgres::{r2d2, PostgresConnectionManager};
use std::collections::HashMap;
use std::thread::sleep;
use std::time::Duration;
use tracing::warn;

static CONNECTION_POOL_MAX_SIZE: u32 = 5;

#[derive(Default)]
pub struct PostgresConnectionWrapper {
    configs: HashMap<String, Config>,
    pools: HashMap<String, r2d2::Pool<PostgresConnectionManager<NoTls>>>,
}

pub type PooledConn = PooledConnection<PostgresConnectionManager<NoTls>>;

impl PostgresConnectionWrapper {
    pub fn add_admin_client(&mut self) {
        let key = String::from("admin");
        let config = get_admin_config();
        self.configs.insert(key, config);
    }

    pub fn add_rw_client(&mut self) {
        let key = String::from("read_write");
        let config = get_rw_config();
        self.configs.insert(key, config);
    }

    pub fn fetch_admin_connection(
        &mut self,
    ) -> Result<PooledConnection<PostgresConnectionManager<NoTls>>, Box<dyn std::error::Error>>
    {
        let key = String::from("admin");
        self.get_or_create_connection(key)
    }

    pub fn fetch_rw_connection(
        &mut self,
    ) -> Result<PooledConnection<PostgresConnectionManager<NoTls>>, Box<dyn std::error::Error>>
    {
        let key = String::from("read_write");
        self.get_or_create_connection(key)
    }
}

impl PostgresConnectionWrapper {
    fn get_or_create_connection(
        &mut self,
        key: String,
    ) -> Result<PooledConnection<PostgresConnectionManager<NoTls>>, Box<dyn std::error::Error>>
    {
        let config = self
            .configs
            .get(&key)
            .ok_or_else(|| Box::new(ConfigNotFoundErr { key: key.clone() }))?;

        // PostgresDB is connected by IP, with no DNS resolve concern on cloud
        if self.pools.get(&key).is_none() {
            let manager = PostgresConnectionManager::new(config.clone(), NoTls);
            let pool = r2d2::Pool::builder()
                .max_size(CONNECTION_POOL_MAX_SIZE)
                .build(manager)?;
            self.pools.insert(key.clone(), pool);
        }
        self.get_connection_from_pool(key.as_str())
    }

    fn get_connection_from_pool(
        &self,
        key: &str,
    ) -> Result<PooledConnection<PostgresConnectionManager<NoTls>>, Box<dyn std::error::Error>>
    {
        loop {
            let pool = self.pools.get(key).unwrap();

            match pool.get() {
                Ok(pooled_connection) => {
                    return Ok(pooled_connection);
                }
                Err(e) => {
                    warn!("Failed to get {} connection. Error: {}. Retrying", &key, e);
                    sleep(Duration::from_secs(3));
                }
            }
        }
    }
}

use crate::connection::config::{get_admin_config, get_rw_config};
use crate::connection::postgres_connection_wrapper::PostgresConnectionError::ConfigNotFoundErr;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum PostgresConnectionError {
    #[error("Config not found for client {key}")]
    ConfigNotFoundErr { key: String },
}

#[cfg(test)]
mod tests {
    use crate::connection::config::tests::set_env;
    use crate::connection::postgres_connection_wrapper::PostgresConnectionWrapper;
    use std::ops::DerefMut;

    #[test]
    #[ignore]
    fn test_get_connection_ok() {
        set_env();
        let mut postgres_wrapper = PostgresConnectionWrapper::default();
        postgres_wrapper.add_rw_client();
        let connection = postgres_wrapper.fetch_rw_connection();
        assert!(connection.is_ok());

        postgres_wrapper.add_admin_client();
        let connection = postgres_wrapper.fetch_admin_connection();
        assert!(connection.is_ok());
    }

    #[test]
    fn test_get_connection_failure() {
        set_env();
        let mut postgres_wrapper = PostgresConnectionWrapper::default();
        let connection = postgres_wrapper.fetch_rw_connection();
        assert!(connection.is_err());
    }

    #[test]
    #[ignore]
    fn test_client_execution() {
        set_env();
        let mut postgres_wrapper = PostgresConnectionWrapper::default();
        postgres_wrapper.add_rw_client();
        let connection = postgres_wrapper.fetch_rw_connection();
        assert!(connection.is_ok());

        let mut client = connection.unwrap();
        let client = client.deref_mut();

        let table = "test_client_execution";
        let create_res = client.execute(
            &format!("CREATE TABLE IF NOT EXISTS {} (id INT)", table),
            &[],
        );
        assert!(create_res.is_ok());

        let query_res = client.simple_query(&format!("SELECT * FROM {} LIMIT 1", table));
        assert!(query_res.is_ok());
        assert_eq!(query_res.unwrap().len(), 1);

        let drop_res = client.execute(&format!("DROP TABLE {}", table), &[]);
        assert!(drop_res.is_ok());
    }
}
