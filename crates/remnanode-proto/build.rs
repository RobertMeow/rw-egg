fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::configure()
        .build_server(false)
        .compile_protos(
            &[
                "proto/app/proxyman/command/command.proto",
                "proto/app/stats/command/command.proto",
                "proto/app/router/command/command.proto",
            ],
            &["proto/"],
        )?;
    Ok(())
}
