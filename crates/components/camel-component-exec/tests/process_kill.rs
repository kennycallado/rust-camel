use camel_component_exec::process::{kill_tree, spawn};
use std::collections::HashMap;

#[tokio::test]
async fn timeout_kills_process_tree() {
    let exe = which("sleep").expect("sleep must be on PATH for this test");
    let mut child = spawn(
        &exe,
        &["30".to_string()],
        &HashMap::new(),
        std::path::Path::new("/tmp"),
    )
    .expect("spawn failed");
    // simulate timeout fire
    kill_tree(&mut child);
    let status = tokio::time::timeout(std::time::Duration::from_secs(5), child.wait()).await;
    assert!(status.is_ok(), "child must be killed, not hang");
}

fn which(name: &str) -> Option<std::path::PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths).find_map(|d| {
            let c = d.join(name);
            if c.is_file() {
                c.canonicalize().ok()
            } else {
                None
            }
        })
    })
}
