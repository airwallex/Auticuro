// Copyright 2020 TiKV Project Authors. Licensed under Apache-2.0.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // tonic build version derive serialize and deserialize
    let proto_list = &["proto/tikvpb.proto", "proto/kvrpcpb.proto"];
    tonic_build::configure()
        .build_server(false)
        .out_dir("src")
        .compile(proto_list, &["./include", "proto/"])
        .unwrap();

    Ok(())
}
