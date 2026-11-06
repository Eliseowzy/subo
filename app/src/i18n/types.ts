import type base from "./locales/zh-CN";

/** 一条文案：普通字符串，或按数量变形（英文的 1 error / 2 errors），数量从 vars.count 取 */
export type Plural = { one?: string; two?: string; few?: string; many?: string; zero?: string; other: string };
export type Msg = string | Plural;
export type Vars = Record<string, string | number>;

/** 把基准语言的结构放宽：叶子可以是字符串或复数形式，键一个都不能少 */
type Widen<T> = { [K in keyof T]: T[K] extends string ? Msg : Widen<T[K]> };
export type Messages = Widen<(typeof base)["messages"]>;

/** 所有合法的文案键，点号路径：t("home.recent") */
type Paths<T, P extends string = ""> = {
  [K in keyof T & string]: T[K] extends string ? `${P}${K}` : Paths<T[K], `${P}${K}.`>;
}[keyof T & string];
export type Key = Paths<(typeof base)["messages"]>;

export type Locale = {
  /** BCP 47 语言代码，与文件名一致；也用作 <html lang>、Intl 的区域 */
  code: string;
  /** 用这种语言自己写的名字，语言菜单里显示（「English」而不是「英语」） */
  name: string;
  messages: Messages;
  /**
   * 宏包说明的译文：宏包名 → 一句话说明。可选，没写的宏包显示 TeX Live 数据库里的英文原文。
   * 内置语言的放在 ../packages/<代码>.json，不进 messages（键是宏包名，数量随 TeX Live 变）。
   */
  packages?: Record<string, string>;
  /** 用户导入的语言包（不是随软件发布的） */
  custom?: boolean;
};

/** 非基准语言文件用它包一层，缺键、多键都会在 tsc 时报出来 */
export const defineLocale = (l: Locale) => l;
