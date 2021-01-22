use anyhow::{Error, Result};
// Serialized version of the Cargo.lock file
static COMPRESSED_DEPENDENCY_LIST: &[u8] = auditable::inject_dependency_list!();

pub(crate) fn get_auditable_dependency_list() -> Result<String> {
    use miniz_oxide::inflate::decompress_to_vec_zlib;
    let decompressed_data = decompress_to_vec_zlib(&COMPRESSED_DEPENDENCY_LIST)
        .map_err(|_| Error::msg("Could not decompress dependency list"))?;
    //.map_err(|_| ("Failed to decompress audit data"))?;
    Ok(String::from_utf8(decompressed_data)?)
}
