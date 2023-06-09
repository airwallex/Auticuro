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
use crate::errors::balance_operation_error::Error as BalanceOperationError;
use crate::errors::general_error::GeneralResult;
use bigdecimal::BigDecimal;
use std::cmp::Ordering;
use std::ops::Neg;
use std::str::FromStr;

/// A wrapper of `BigDecimal` to support calculations between arbitrary precision numbers
pub struct BalanceCalculator;

impl BalanceCalculator {
    /// Add two numbers, represented by type `&str`
    /// Example: "12.34" + "0.020" = "12.36"
    pub fn add(&self, lhs: &str, rhs: &str) -> GeneralResult<String> {
        let lhs_decimal = BigDecimal::from_str(lhs)
            .map_err(|err| BalanceOperationError::ParseError(lhs.to_string(), err))?;
        let rhs_decimal = BigDecimal::from_str(rhs)
            .map_err(|err| BalanceOperationError::ParseError(rhs.to_string(), err))?;
        let result = lhs_decimal + rhs_decimal;
        // Normalized to save space, e.g. 12.230000 => 12.23
        Ok(result.normalized().to_string())
    }

    /// Calculate the negative of a number, represented by type `&str`
    /// Example: neg("-12.34") = "12.34"
    pub fn neg(&self, input: &str) -> GeneralResult<String> {
        Ok(BigDecimal::from_str(input)
            .map_err(|err| BalanceOperationError::ParseError(input.to_string(), err))?
            .neg()
            .to_string())
    }

    /// Compare two numbers, represented by type `&str`
    pub fn cmp(&self, lhs: &str, rhs: &str) -> GeneralResult<Ordering> {
        let lhs_decimal = BigDecimal::from_str(lhs)
            .map_err(|err| BalanceOperationError::ParseError(lhs.to_string(), err))?;
        let rhs_decimal = BigDecimal::from_str(rhs)
            .map_err(|err| BalanceOperationError::ParseError(rhs.to_string(), err))?;
        Ok(lhs_decimal.cmp(&rhs_decimal))
    }
}
