#[cfg(not(feature = "server"))]
compile_error!("server feature is required for the server_binary test");

#[cfg(feature = "server")]
#[test]
fn server_binary_is_built_with_server_feature() {
    let path = assert_cmd::cargo::cargo_bin!("moviebox-server");
    assert!(path.exists());
}
