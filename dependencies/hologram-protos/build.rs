// Generate Tonic interface based on protos.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    do_tonic_build();

    do_protobuf_build();

    Ok(())
}

fn do_tonic_build() {
    // build firm_wallet/ledger_service for consensus stress test
    tonic_build_firm_wallet();
}

fn do_protobuf_build() {
    // build firm_wallet/ledger service
    protobuf_build_common(
        "src/proto/firm_wallet",
        &["src/proto/"],
        "src/firm_walletpb",
    );
}

fn protobuf_build_common(path: &str, include: &[&str], out_dir: &str) {
    use protobuf_build::Builder as ProtobufBuilder;

    ProtobufBuilder::new()
        .search_dir_for_protos(path)
        .out_dir(out_dir)
        .includes(include)
        .generate();
}

fn tonic_build_firm_wallet() {
    let proto_list = &[
        "src/proto/firm_wallet/accountpb.proto",
        "src/proto/firm_wallet/errorpb.proto",
        "src/proto/firm_wallet/commonpb.proto",
        "src/proto/firm_wallet/account_management_servicepb.proto",
        "src/proto/firm_wallet/balance_operation_servicepb.proto",
    ];
    default_tonic_build(proto_list);

    let proto_list_need_server_build = &["src/proto/firm_wallet/internal_servicepb.proto"];
    tonic_build(proto_list_need_server_build, true);
}

fn default_tonic_build(proto_list: &[&str]) {
    tonic_build(proto_list, false);
}

fn tonic_build(proto_list: &[&str], build_server: bool) {
    tonic_build::configure()
        .type_attribute(".", "#[derive(serde::Serialize, serde::Deserialize)]")
        .build_server(build_server)
        .format(false)
        .compile(proto_list, &["./src/proto"])
        .unwrap();
}
