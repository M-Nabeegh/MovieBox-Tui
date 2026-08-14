#[test]
fn server_binary_is_built_with_server_feature() {
    let path = assert_cmd::cargo::cargo_bin!("moviebox-server");
    assert!(path.exists());
}
