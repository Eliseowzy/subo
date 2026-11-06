import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { bootAppearance } from "./appearance";
import { installContextMenuGuard } from "./ContextMenu";
import { installTruncationTitles } from "./ui";
import "./styles.css";

installContextMenuGuard();
installTruncationTitles();

// Windows 的字体、滚动条和 macOS 不一样，样式里用 html[data-os="win"] 单独调
if (/Win/.test(navigator.platform)) document.documentElement.dataset.os = "win";

// 先取回外观与语言（本机服务，毫秒级；连不上最多等 1.5 秒），免得首屏先用默认配色和语言再跳一下
void bootAppearance().finally(() =>
  ReactDOM.createRoot(document.getElementById("root")!).render(
    <React.StrictMode>
      <App />
    </React.StrictMode>
  )
);
