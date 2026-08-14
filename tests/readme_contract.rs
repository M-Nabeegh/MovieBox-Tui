use std::{fs, path::Path};

#[test]
fn readme_publishes_self_hosted_server_contract() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("README.md");
    let readme = fs::read_to_string(path).expect("README.md should be readable");
    for required in [
        "independently maintained fork", "Docker", "admin_password.txt",
        "session_key.txt", "docker compose", "Jellyfin", "Tailscale", "LAN",
        "1080p", "/mnt/nas-data/moviebox", "backup", "uninstall",
        "Troubleshooting", "MIT", "content",
    ] {
        assert!(readme.contains(required), "README is missing {required:?}");
    }
}
