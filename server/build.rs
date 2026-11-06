//! 把构建时的 git 提交号和提交日期编进程序，「关于」里显示，排查问题时知道是哪一版。
//! 没有 git（源码包构建）就留空，界面上只显示版本号。

use std::process::Command;

fn git(args: &[&str]) -> String {
    Command::new("git")
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

fn main() {
    println!("cargo:rustc-env=SUBO_BUILD={}", git(&["rev-parse", "--short", "HEAD"]));
    println!("cargo:rustc-env=SUBO_BUILD_DATE={}", git(&["log", "-1", "--format=%cs"]));
    // 换提交时重跑：HEAD 指向的分支动了，或者 HEAD 本身换了
    let head = git(&["rev-parse", "--git-path", "HEAD"]);
    if !head.is_empty() {
        println!("cargo:rerun-if-changed={head}");
    }
    let branch = git(&["symbolic-ref", "-q", "HEAD"]);
    if !branch.is_empty() {
        let r = git(&["rev-parse", "--git-path", &branch]);
        println!("cargo:rerun-if-changed={r}");
    }
    println!("cargo:rerun-if-changed=build.rs");
}
