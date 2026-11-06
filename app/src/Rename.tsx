import { useEffect, useId, useRef, useState } from "react";
import { NAME_MAX } from "./api";
import Icon from "./Icon";
import { LAYOUT_DRAG } from "./Split";
import { t, useLocale } from "./i18n";

/** 名字的字数，和服务端一样按字符算（不是 UTF-16 码元，emoji 也算一个） */
export const nameLength = (v: string) => [...v.trim()].length;

/**
 * 快到上限时显示字数，超了标红。输入框本身不设 maxLength：
 * 粘贴一长段被悄悄砍掉，用户会以为存进去的是全文。
 */
export function NameHint({ value, max = NAME_MAX }: { value: string; max?: number }) {
  useLocale();
  const n = nameLength(value);
  if (n <= max - 10) return null;
  const over = n > max;
  return (
    <span className={`name-hint${over ? " over" : ""}`} onClick={(e) => e.stopPropagation()}>
      {t(over ? "rename.over" : "rename.count", { n, max })}
    </span>
  );
}

type Props = {
  initial: string;
  className?: string;
  /** 回车或失焦提交；名字没变或为空时不调用 */
  onCommit: (name: string) => Promise<void>;
  onCancel: () => void;
  /** 输入框后面带对勾 / 叉按钮，失焦什么都不做：只有回车、对勾提交，Esc、叉放弃 */
  confirm?: boolean;
  /** 字数上限，默认项目名的 NAME_MAX；文件名放宽 */
  max?: number;
  /** 打开时只选中这一段（文件名不选扩展名）；默认全选 */
  select?: [number, number];
};

/** 原地改名的输入框：回车提交，Esc 取消，失焦也算提交（confirm 模式除外）。提交失败在输入框下面冒一条错误，读屏也会念。 */
export default function RenameInput({ initial, className, onCommit, onCancel, confirm, max = NAME_MAX, select }: Props) {
  useLocale();
  const [value, setValue] = useState(initial);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const errorId = useId();
  const ref = useRef<HTMLInputElement>(null);
  // 回车提交后输入框卸载时还会触发一次 blur，别再提交第二遍
  const done = useRef(false);

  useEffect(() => {
    ref.current?.focus();
    if (select) ref.current?.setSelectionRange(...select);
    else ref.current?.select();
    // 只在挂上时选一次
  }, []);

  // 拖分栏调布局时放弃改名（拖动条拦了焦点转移，不会失焦）
  const cancelRef = useRef(onCancel);
  cancelRef.current = onCancel;
  useEffect(() => {
    const drop = () => {
      if (done.current) return;
      done.current = true;
      cancelRef.current();
    };
    window.addEventListener(LAYOUT_DRAG, drop);
    return () => window.removeEventListener(LAYOUT_DRAG, drop);
  }, []);

  const commit = async () => {
    if (done.current || busy) return;
    const name = value.trim();
    if (!name || name === initial) {
      done.current = true;
      onCancel();
      return;
    }
    if (nameLength(name) > max) {
      setError(t("rename.tooLong", { max }));
      return;
    }
    setBusy(true);
    try {
      done.current = true;
      await onCommit(name);
    } catch (e) {
      done.current = false;
      setError(String(e).replace(/^Error:\s*/, ""));
      setBusy(false);
      ref.current?.focus();
    }
  };

  const over = nameLength(value) > max;
  const cancel = () => {
    done.current = true;
    onCancel();
  };

  return (
    <>
      <span className={`rename-field${className ? ` ${className}-field` : ""}`}>
      <input
        ref={ref}
        className={`input rename-input${error || over ? " invalid" : ""}${className ? ` ${className}` : ""}`}
        value={value}
        disabled={busy}
        title={t("rename.inputTitle")}
        aria-label={t("rename.label")}
        aria-invalid={!!error || over}
        aria-describedby={error ? errorId : undefined}
        onChange={(e) => {
          setValue(e.target.value);
          setError("");
        }}
        onClick={(e) => e.stopPropagation()}
        onKeyDown={(e) => {
          e.stopPropagation();
          if (e.key === "Enter") void commit();
          if (e.key === "Escape") cancel();
        }}
        // 出过错或超长时失焦就当放弃，免得同一个错误反复弹；confirm 模式失焦不动，等用户点对勾或叉
        onBlur={confirm ? undefined : () => (error || over ? onCancel() : void commit())}
      />
      {error && (
        <span className="rename-error" id={errorId} role="alert">
          <Icon name="error" size={13} />
          {error}
        </span>
      )}
      </span>
      {confirm && (
        <span className="rename-buttons" onClick={(e) => e.stopPropagation()}>
          <button
            type="button"
            className="btn btn-ghost btn-sm btn-icon rename-ok"
            title={t("rename.save")}
            aria-label={t("rename.save")}
            disabled={busy || over || !!error}
            onClick={() => void commit()}
          >
            <Icon name="check" size={15} />
          </button>
          <button
            type="button"
            className="btn btn-ghost btn-sm btn-icon"
            title={t("rename.discard")}
            aria-label={t("rename.discard")}
            disabled={busy}
            onClick={cancel}
          >
            <Icon name="x" size={15} />
          </button>
        </span>
      )}
      <NameHint value={value} max={max} />
    </>
  );
}
