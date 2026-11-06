use std::{
    io::Write,
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
};

/// Private pipe to a software-painted Qt window; no listener or plaintext files.
pub struct GraphViewer(Mutex<Option<Child>>);
impl GraphViewer {
    pub fn open() -> Arc<Self> {
        let child = if std::env::var_os("DISPLAY").is_some()
            || std::env::var_os("WAYLAND_DISPLAY").is_some()
        {
            Command::new("python3")
                .args(["-c", include_str!("../scripts/graph_view.py")])
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .ok()
        } else {
            None
        };
        Arc::new(Self(Mutex::new(child)))
    }
    pub fn publish(&self, data: &serde_json::Value) {
        if let Ok(mut guard) = self.0.lock() {
            if let Some(child) = guard.as_mut() {
                let failed = child.stdin.as_mut().is_none_or(|pipe| {
                    serde_json::to_writer(&mut *pipe, data).is_err()
                        || pipe.write_all(b"\n").is_err()
                });
                if failed {
                    let _ = child.kill();
                    let _ = child.wait();
                    *guard = None;
                }
            }
        }
    }
    pub fn close(&self) {
        if let Ok(mut guard) = self.0.lock() {
            if let Some(mut child) = guard.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}
