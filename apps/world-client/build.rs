fn main() {
    let proto = "../../Epic_Game_V/proto/economy/v1/economy.proto";
    println!("cargo:rerun-if-changed={proto}");
    prost_build::compile_protos(&[proto], &["../../Epic_Game_V/proto"]).unwrap();
}
