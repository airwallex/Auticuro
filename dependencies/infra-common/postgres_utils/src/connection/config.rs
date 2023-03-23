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
use dotenv::dotenv;
use r2d2_postgres::postgres::Config;
use std::env;

fn get_pg_config(username: &str, password: &str) -> Config {
    dotenv().ok();

    let mut config = Config::new();
    let host = env::var("POSTGRES_HOST").expect("Postgres host must be set");
    let port = env::var("POSTGRES_PORT")
        .expect("Postgres port must be set")
        .parse::<u16>()
        .expect("Port must be a number");
    let dbname = env::var("POSTGRES_DATABASE").expect("Postgres database name must be set");

    let application = env::var("APPLICATION").unwrap_or_default();

    config
        .host(host.as_str())
        .port(port)
        .dbname(dbname.as_str())
        .user(username)
        .password(password.as_bytes())
        .application_name(application.as_str());

    config
}

pub fn get_rw_config() -> Config {
    let username = env::var("POSTGRES_USERNAME").expect("Postgres username must be set");
    let password = env::var("POSTGRES_PASSWORD").expect("Postgres password must be set");
    get_pg_config(&username, &password)
}

pub fn get_admin_config() -> Config {
    let username = env::var("FLYWAY_USERNAME").expect("Flyway username must be set");
    let password = env::var("FLYWAY_PASSWORD").expect("Flyway password must be set");
    get_pg_config(&username, &password)
}

#[cfg(test)]
pub mod tests {
    use crate::connection::config::{get_admin_config, get_rw_config};
    use std::convert::TryFrom;
    use std::env;

    pub fn set_env() {
        env::set_var("POSTGRES_HOST", "localhost");
        env::set_var("POSTGRES_PORT", "5432");
        env::set_var("POSTGRES_DATABASE", "pixiueventstore");
        env::set_var("POSTGRES_USERNAME", "postgres_rw");
        env::set_var("POSTGRES_PASSWORD", "postgres_rw");
        env::set_var("FLYWAY_USERNAME", "postgres_admin");
        env::set_var("FLYWAY_PASSWORD", "postgres_admin");
    }
    #[test]
    #[ignore]
    fn test_get_rw_config() {
        set_env();
        let rw_config = get_rw_config();

        assert_eq!(rw_config.get_user().unwrap(), "postgres_rw");
        assert_eq!(
            String::from_utf8(Vec::try_from(rw_config.get_password().unwrap()).unwrap()).unwrap(),
            "postgres_rw"
        );

        assert_eq!(rw_config.get_dbname().unwrap(), "pixiueventstore");
    }

    #[test]
    #[ignore]
    fn test_get_admin_config() {
        set_env();
        let rw_config = get_admin_config();
        assert_eq!(rw_config.get_user().unwrap(), "postgres_admin");
        assert_eq!(
            String::from_utf8(Vec::try_from(rw_config.get_password().unwrap()).unwrap()).unwrap(),
            "postgres_admin"
        );
    }
}
