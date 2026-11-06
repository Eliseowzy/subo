//! 命令行入口：
//!   subo-server --port 8787          # 项目库默认在 ~/Documents/Subo
//!   subo-server --bundled-tex …      # 补装宏包走 tlmgr --usermode（同桌面壳的内置 TeX）
//!
//!   subo-server build ./paper …      # 一次性命令（build、install、doctor……），不起服务，见 `cli.rs`
//!
//! 服务本体在库里（`lib.rs`），桌面壳也用它。命令行与 HTTP 接口的用法见 docs/cli.md。

use std::{net::SocketAddr, path::PathBuf, process::exit, time::Duration};

const USAGE: &str = "\
用法: subo-server [选项]                启动本地服务
      subo-server <命令> …              一次性命令：build、install、doctor、inspect、mirror、templates、new
                                        （subo-server help 看命令说明）

  --port <n>       监听 127.0.0.1:<n>，默认 8787；0 表示随机空闲端口（看启动输出里的「地址」）
  --home <dir>     项目库目录（给了就锁定，设置里不能再改）
  --dist <dir>     同时托管前端构建产物（app/dist）
  --engine <e>     auto | xelatex | tectonic | mock，默认 auto
  --timeout <秒>   单次编译超时，默认 120
  --bundled-tex    按桌面端内置 TeX Live 行事（补装宏包装进用户层）
  --bundled-root <dir>
                   同桌面端选 TeX：<dir> 是内置 TeX Live 的根目录，PATH 里的 TeX 缺引擎或太旧时用它，
                   设置里能换（PATH、TEXMFHOME 不用自己配）；none 表示没有内置版
  -h, --help       显示本说明

环境变量 SUBO_HOME 改 Subo 的配置与缓存目录（默认 ~/.subo）；测试时设成临时目录。
接口文档: docs/cli.md";

fn die(msg: &str) -> ! {
    eprintln!("subo-server: {msg}\n\n{USAGE}");
    exit(2);
}

#[tokio::main]
async fn main() {
    // 第一个参数是命令（build、install……）就走一次性的命令行，不起服务
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.first().is_some_and(|a| subo_server::cli::wants(a)) {
        let bundled = argv.iter().any(|a| a == "--bundled-tex");
        let rest: Vec<String> = argv.into_iter().filter(|a| a != "--bundled-tex").collect();
        exit(tokio::task::block_in_place(|| subo_server::cli::run("subo-server", &rest, bundled)));
    }

    let mut opts = subo_server::Options::default();
    let mut port: u16 = 8787;
    let mut bundled_root: Option<PathBuf> = None;
    let mut own_only = false;

    let mut args = argv.into_iter();
    while let Some(flag) = args.next() {
        let mut value = |name: &str| args.next().unwrap_or_else(|| die(&format!("{name} 缺参数")));
        match flag.as_str() {
            "--home" => opts.home = Some(PathBuf::from(value("--home"))),
            "--port" => {
                let v = value("--port");
                port = v.parse().unwrap_or_else(|_| die(&format!("--port 不是合法端口: {v}")));
            }
            "--engine" => {
                let v = value("--engine");
                if !matches!(v.as_str(), "auto" | "xelatex" | "tectonic" | "mock") {
                    die(&format!("--engine 不认识: {v}"));
                }
                opts.engine = v;
            }
            "--dist" => {
                let v = PathBuf::from(value("--dist"));
                if !v.join("index.html").is_file() {
                    die(&format!("--dist 下没有 index.html: {}（先在 app/ 下 npm run build）", v.display()));
                }
                opts.dist = Some(v);
            }
            "--timeout" => {
                let v = value("--timeout");
                let secs: u64 = v.parse().ok().filter(|&n| n > 0).unwrap_or_else(|| die(&format!("--timeout 不是正整数: {v}")));
                opts.timeout = Duration::from_secs(secs);
            }
            // 按桌面壳的内置 TeX Live 行事（补装走用户层）。PATH、TEXMFHOME 由调用方配好，
            // scripts/bundled-smoke.sh 用它在终端里复现桌面端
            "--bundled-tex" => opts.bundled_tex = true,
            "--bundled-root" => {
                let v = value("--bundled-root");
                // none：同桌面端选 TeX，但没有内置版（复现不带 TeX 的 Windows 安装包）
                let root = PathBuf::from(&v);
                if v != "none" && !root.join("bin").is_dir() {
                    die(&format!("--bundled-root 下没有 bin 目录: {}", root.display()));
                }
                own_only = v == "none";
                bundled_root = (v != "none").then_some(root);
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                return;
            }
            other => die(&format!("不认识的参数: {other}")),
        }
    }

    if bundled_root.is_some() || own_only {
        subo_server::tex::init(bundled_root);
    }

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .unwrap_or_else(|e| die(&format!("端口 {port} 绑不上: {e}（已有 subo-server 在跑？换个 --port，或用 0）")));
    subo_server::serve(opts, listener).await.expect("服务异常退出");
}
