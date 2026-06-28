fn main() -> Result<(), Box<dyn std::error::Error>> {
    let proto_dir = std::env::var("CARGO_MANIFEST_DIR")
        .map(|d| std::path::PathBuf::from(d).join("proto"))
        .unwrap();

    tonic_prost_build::configure()
        .build_server(false)
        .build_client(true)
        .compile_protos(
            &[&proto_dir.join("buildkit/control.proto")],
            &[&proto_dir],
        )
        .unwrap();

    Ok(())
}
