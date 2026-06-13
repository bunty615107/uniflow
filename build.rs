// Build script to compile the rclone_bridge.proto into Rust code with tonic.
// Run `cargo build` to generate the client code in target or OUT_DIR.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::compile_protos("proto/rclone_bridge.proto")?;
    Ok(())
}