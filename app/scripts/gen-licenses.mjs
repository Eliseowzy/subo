// 第三方许可清单：随软件分发的 npm 包（前端）和 Rust crate（本地服务、桌面壳），
// 写到 public/licenses.json，「关于 → 第三方许可」读它。npm run build 会先跑这里。
//
// Rust 那份靠 cargo metadata（离线，读本机 crate 缓存）。找不到 cargo 时只出 npm 部分并警告；
// 打安装包走 scripts/desktop.sh，那里有 cargo，清单是全的。

import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const app = fileURLToPath(new URL("..", import.meta.url));
const desktop = join(app, "..", "desktop", "Cargo.toml");

/** 只算运行时依赖（不含 devDependencies），自己的包不列 */
function npm() {
  const tree = JSON.parse(execFileSync("npm", ["ls", "--omit=dev", "--all", "--json"], { cwd: app, encoding: "utf8" }));
  const seen = new Map();
  const walk = (deps) => {
    for (const [name, d] of Object.entries(deps ?? {})) {
      const key = `${name}@${d.version}`;
      if (!d.version || seen.has(key)) continue;
      let license = "";
      try {
        const pkg = JSON.parse(readFileSync(join(app, "node_modules", name, "package.json"), "utf8"));
        license = typeof pkg.license === "string" ? pkg.license : pkg.license?.type ?? "";
      } catch {
        /* 嵌套安装的同名包不在顶层，许可证与顶层那份一致的居多，留空让人看见 */
      }
      seen.set(key, { name, version: d.version, license, source: "npm" });
      walk(d.dependencies);
    }
  };
  walk(tree.dependencies);
  return [...seen.values()];
}

/** 桌面壳在本机平台上实际编进去的 crate；路径依赖（素帛自己的）不列 */
function cargo() {
  const host = execFileSync("rustc", ["-vV"], { encoding: "utf8" }).match(/^host: (.+)$/m)[1];
  const meta = JSON.parse(
    execFileSync(
      "cargo",
      ["metadata", "--format-version", "1", "--offline", "--filter-platform", host, "--manifest-path", desktop],
      { encoding: "utf8", maxBuffer: 64 << 20 }
    )
  );
  const used = new Set(meta.resolve.nodes.map((n) => n.id));
  return meta.packages
    .filter((p) => p.source && used.has(p.id))
    .map((p) => ({ name: p.name, version: p.version, license: p.license ?? "", source: "cargo" }));
}

let rust = [];
try {
  rust = cargo();
} catch (e) {
  console.warn(`gen-licenses: 没取到 Rust 依赖（${String(e.message).split("\n")[0]}），清单里只有 npm 部分`);
}
const list = [...npm(), ...rust].sort((a, b) => a.source.localeCompare(b.source) || a.name.localeCompare(b.name));
writeFileSync(join(app, "public", "licenses.json"), JSON.stringify(list));
console.log(`gen-licenses: npm ${list.length - rust.length} 个，Rust ${rust.length} 个`);
