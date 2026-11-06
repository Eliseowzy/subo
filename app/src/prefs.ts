import { api, type Settings } from "./api";

/*
 * 服务端设置的前端缓存。设置页之外只有少数地方要读（缺字清单看「自动安装」），
 * 读一次记下来；设置页保存后用 remember() 更新，不用再请求。
 */

let current: Settings | null = null;
let loading: Promise<Settings> | null = null;

export function prefs(): Promise<Settings> {
  if (current) return Promise.resolve(current);
  loading ??= api
    .settings()
    .then((r) => (current = r.settings))
    .finally(() => (loading = null));
  return loading;
}

export function remember(s: Settings) {
  current = s;
}
