use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=escrow/src/lib.rs");
    let generated = spel_client_gen::generate_from_idl_json(lez_zec_escrow_v02::PROGRAM_IDL_JSON)
        .expect("the exact pinned SPEL generator must accept the generated escrow IDL");
    let destination = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo provides OUT_DIR"))
        .join("zec_escrow_client.rs");
    fs::write(&destination, generated.client_code)
        .expect("write the ephemeral generated client into OUT_DIR");
    let wrapper = destination.with_file_name("zec_escrow_client_module.rs");
    let wrapper_source = format!(
        "#[allow(\n    clippy::too_many_arguments,\n    dead_code,\n    unused_imports,\n    unused_mut\n)]\n#[path = {destination:?}]\nmod exact_generated_client;\npub use exact_generated_client::*;\n"
    );
    fs::write(wrapper, wrapper_source).expect("write the ephemeral module wrapper into OUT_DIR");
}
