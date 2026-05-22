use std::path::PathBuf;
use std::env;

fn main() -> anyhow::Result<()> {
    let out_dir = PathBuf::from(env::var("OUT_DIR")?);

    // Generate user messages from HIROZ_MSG_PATH environment variable
    // The generated code will reference standard types (geometry_msgs, builtin_interfaces)
    // from hiroz_msgs using fully qualified paths
    hiroz_codegen::generate_user_messages(&out_dir, false)?;

    println!("cargo:rerun-if-env-changed=HIROZ_MSG_PATH");
    Ok(())
}
