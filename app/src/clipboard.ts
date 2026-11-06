// 剪贴板读写集中在这里，平台差异只改这一处。
// 页面由本地服务经 http://127.0.0.1 提供，属于安全上下文，标准 Clipboard API 可用：
//   - macOS WKWebView：读剪贴板会先弹系统的「粘贴」确认气泡，点了才给内容
//   - Windows WebView2 / Linux WebKitGTK：通常直接可读，被拒时同样走失败分支
// 以后壳若提供原生剪贴板（Tauri clipboard 插件等），在这里优先用它即可。

/**
 * 用当前焦点元素的选区做系统复制 / 剪切。编辑器里会走 CodeMirror 自己的 copy/cut 处理（多选区、整行复制都对），
 * 同步执行、不需要剪贴板权限，WKWebView、WebView2、WebKitGTK 都支持；不支持时返回 false，再退回 writeText。
 */
export function execCopy(cut: boolean): boolean {
  try {
    return document.execCommand(cut ? "cut" : "copy");
  } catch {
    return false;
  }
}

export async function writeText(text: string): Promise<void> {
  await navigator.clipboard.writeText(text);
}

/** 读不到（用户取消、WebView 不允许）时抛错，调用方提示改用快捷键 */
export async function readText(): Promise<string> {
  if (!navigator.clipboard?.readText) throw new Error("clipboard read unavailable");
  return navigator.clipboard.readText();
}
