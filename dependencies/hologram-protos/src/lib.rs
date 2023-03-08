pub mod firm_walletpb;

// For tonic builder, used in consensus-stress-test
pub mod firm_wallet {
    pub mod errorpb {
        include!(concat!(env!("OUT_DIR"), "/firm_wallet.errorpb.rs"));
    }

    pub mod commonpb {
        include!(concat!(env!("OUT_DIR"), "/firm_wallet.commonpb.rs"));
    }

    pub mod accountpb {
        include!(concat!(env!("OUT_DIR"), "/firm_wallet.accountpb.rs"));
    }

    pub mod account_management_servicepb {
        include!(concat!(
            env!("OUT_DIR"),
            "/firm_wallet.account_management_servicepb.rs"
        ));
    }

    pub mod balance_operation_servicepb {
        include!(concat!(
            env!("OUT_DIR"),
            "/firm_wallet.balance_operation_servicepb.rs"
        ));
    }

    pub mod internal_servicepb {
        include!(concat!(
            env!("OUT_DIR"),
            "/firm_wallet.internal_servicepb.rs"
        ));
    }
}