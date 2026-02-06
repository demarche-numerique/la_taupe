use assert_cmd::prelude::*;
use core::time;
use std::{
    process::{Child, Command},
    thread,
};

#[allow(dead_code)]
pub struct TaupeAndNode {
    taupe: ChildGuard,
    node: ChildGuard,
}

impl TaupeAndNode {
    pub fn start() -> TaupeAndNode {
        TaupeAndNode::start_with_timeout(None)
    }

    pub fn start_with_timeout(timeout_secs: Option<u64>) -> TaupeAndNode {
        let mut cmd = Command::cargo_bin("la_taupe").unwrap();

        if let Some(secs) = timeout_secs {
            cmd.env("LA_TAUPE_ANALYSIS_TIMEOUT_SECS", secs.to_string());
        }

        let child = cmd.spawn().expect("failed to execute la_taupe");

        let taupe = ChildGuard {
            child,
            description: "la_taupe",
        };

        let child = Command::new("node")
            .arg("server.js")
            .env("DEBUG", "express:*")
            .current_dir("tests/fixtures/static_server")
            .spawn()
            .expect("failed to execute node");

        let node = ChildGuard {
            child,
            description: "node",
        };

        thread::sleep(time::Duration::from_secs(4));

        TaupeAndNode { taupe, node }
    }
}

pub struct ChildGuard {
    pub child: Child,
    description: &'static str,
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        println!(
            "ChildGuard: killing out-of-scope '{}' process",
            self.description
        );
        match self.child.kill() {
            Err(e) => println!(
                "ChildGuard: could not kill out-of-scope '{}' process: {}",
                self.description, e
            ),
            Ok(_) => println!(
                "ChildGuard: successfully killed out-of-scope '{}' process",
                self.description
            ),
        }
    }
}
