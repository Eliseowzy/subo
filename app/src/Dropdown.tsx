// 设置页的下拉框：按钮长得和输入框一样，点开是应用自己的浮动菜单（跟主题走），不用系统原生下拉
// 当前项打勾，菜单至少和按钮一样宽、贴在按钮正下方

import { useState, type MouseEvent } from "react";
import ContextMenu from "./ContextMenu";
import Icon from "./Icon";

export type DropdownOption = { value: string; label: string; lang?: string };

type Props = {
  value: string;
  options: DropdownOption[];
  onChange: (value: string) => void;
  /** 读屏用的名字 */
  label: string;
};

export default function Dropdown({ value, options, onChange, label }: Props) {
  const [menu, setMenu] = useState<{ x: number; y: number; w: number } | null>(null);
  const cur = options.find((o) => o.value === value);
  const toggle = (e: MouseEvent<HTMLButtonElement>) => {
    if (menu) return setMenu(null);
    const r = e.currentTarget.getBoundingClientRect();
    setMenu({ x: r.left, y: r.bottom + 4, w: r.width });
  };
  return (
    <>
      <button
        type="button"
        className={`dropdown${menu ? " open" : ""}`}
        onClick={toggle}
        // 菜单开着时按下按钮：先别让菜单的「点外面就收」生效，交给 toggle 收
        onPointerDown={(e) => menu && e.stopPropagation()}
        aria-haspopup="menu"
        aria-expanded={!!menu}
        aria-label={label}
      >
        <span className="dropdown-label" lang={cur?.lang}>
          {cur?.label ?? value}
        </span>
        <Icon name="chevronDown" size={14} className="dropdown-caret" />
      </button>
      {menu && (
        <ContextMenu
          x={menu.x}
          y={menu.y}
          minWidth={menu.w}
          label={label}
          items={options.map((o) => ({
            icon: o.value === value ? "check" : undefined,
            checked: o.value === value,
            label: o.label,
            run: () => o.value !== value && onChange(o.value),
          }))}
          onClose={() => setMenu(null)}
        />
      )}
    </>
  );
}
