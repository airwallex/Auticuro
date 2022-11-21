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
use gateway_framework::AbstractNode;
use hologram_protos::firm_wallet::account_management_servicepb::account_management_service_client::AccountManagementServiceClient;
use hologram_protos::firm_wallet::balance_operation_servicepb::balance_operation_service_client::BalanceOperationServiceClient;
use tonic::transport::Channel;
use tower::timeout::Timeout;

#[derive(Clone)]
pub struct Node {
    pub balance_operation_service_entrypoint: BalanceOperationServiceClient<Timeout<Channel>>,
    pub account_management_service_entrypoint: AccountManagementServiceClient<Timeout<Channel>>,
}

impl AbstractNode for Node {
    fn new(channel: Timeout<Channel>) -> Self {
        Node {
            balance_operation_service_entrypoint: BalanceOperationServiceClient::new(
                channel.clone(),
            ),
            account_management_service_entrypoint: AccountManagementServiceClient::new(channel),
        }
    }
}

pub mod account_management_service_gateway {
    use gateway_framework::impl_gateway;
    use crate::impl_abstract_response;
    use hologram_protos::firm_wallet::errorpb::error::Error;
    use hologram_protos::firm_wallet::account_management_servicepb::account_management_service_server
    ::AccountManagementService;
    use hologram_protos::firm_wallet::account_management_servicepb::{
        CreateAccountRequest, CreateAccountResponse, DeleteAccountRequest, DeleteAccountResponse,
        GetAccountRequest, GetAccountResponse, LockAccountRequest, LockAccountResponse,
        UnlockAccountRequest, UnlockAccountResponse, UpdateAccountConfigRequest,
        UpdateAccountConfigResponse,
    };
    use crate::firm_wallet_gateway::{ Node, ResponseWrapper };

    impl_abstract_response!(&CreateAccountResponse);
    impl_abstract_response!(&LockAccountResponse);
    impl_abstract_response!(&UnlockAccountResponse);
    impl_abstract_response!(&DeleteAccountResponse);
    impl_abstract_response!(&UpdateAccountConfigResponse);
    impl_abstract_response!(&GetAccountResponse);

    impl_gateway!(
        AccountManagementService,
        Node,
        account_management_service_entrypoint,
        (
            create_account,
            CreateAccountRequest,
            ResponseWrapper<CreateAccountResponse>
        ),
        (
            lock_account,
            LockAccountRequest,
            ResponseWrapper<LockAccountResponse>
        ),
        (
            unlock_account,
            UnlockAccountRequest,
            ResponseWrapper<UnlockAccountResponse>
        ),
        (
            delete_account,
            DeleteAccountRequest,
            ResponseWrapper<DeleteAccountResponse>
        ),
        (
            update_account_config,
            UpdateAccountConfigRequest,
            ResponseWrapper<UpdateAccountConfigResponse>
        ),
        (
            get_account,
            GetAccountRequest,
            ResponseWrapper<GetAccountResponse>
        )
    );
}

pub mod balance_operation_service_gateway {
    use crate::impl_abstract_response;
    use gateway_framework::impl_gateway;
    use hologram_protos::firm_wallet::errorpb::error::Error;
    use hologram_protos::firm_wallet::balance_operation_servicepb::balance_operation_service_server
    ::BalanceOperationService;
    use hologram_protos::firm_wallet::balance_operation_servicepb::{
        BatchBalanceOperationRequest, BatchBalanceOperationResponse, QueryBalanceRequest,
        QueryBalanceResponse, ReleaseRequest, ReleaseResponse, ReserveRequest, ReserveResponse,
        TransferRequest, TransferResponse,
    };
    use crate::firm_wallet_gateway::{ Node,ResponseWrapper };

    impl_abstract_response!(&TransferResponse);
    impl_abstract_response!(&BatchBalanceOperationResponse);
    impl_abstract_response!(&ReserveResponse);
    impl_abstract_response!(&ReleaseResponse);
    impl_abstract_response!(&QueryBalanceResponse);

    impl_gateway!(
        BalanceOperationService,
        Node,
        balance_operation_service_entrypoint,
        (transfer, TransferRequest, ResponseWrapper<TransferResponse>),
        (
            batch_balance_operation,
            BatchBalanceOperationRequest,
            ResponseWrapper<BatchBalanceOperationResponse>
        ),
        (reserve, ReserveRequest, ResponseWrapper<ReserveResponse>),
        (release, ReleaseRequest, ResponseWrapper<ReleaseResponse>),
        (
            query_balance,
            QueryBalanceRequest,
            ResponseWrapper<QueryBalanceResponse>
        )
    );
}

// To workaround the rust orphan rules, because `AbstractResponse` and grpc response like
// `QueryBalanceResponse` are both in remote crates.
// The real ResponseWrapper object contructed in gateway_framework is ResponseWrapper(&AbstractResponse)
// which avoids copy. In the definition of ResponseWrapper, generic type T instead of &T is used to
// avoid lifetime specification, which will make the macro hard to maintain.
pub struct ResponseWrapper<T>(T);

#[macro_export]
macro_rules! impl_abstract_response {
    ($response_type:ty) => {
        impl AbstractResponse for ResponseWrapper<$response_type> {
            fn has_not_leader_error(&self) -> bool {
                matches!(
                    self.0.error.as_ref().unwrap().error.as_ref(),
                    Some(Error::NotLeader(_))
                )
            }

            fn success(&self) -> bool {
                self.0.error.is_none()
            }
        }
    };
}
