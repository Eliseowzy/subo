// 标志：反斜杠加一个点，几何见 docs/brand/README.md。点就是状态灯：
// busy 时沿斜杠滑下（正在编译），alert 时变琥珀色（出错或缺包）
type Props = { size?: number; busy?: boolean; alert?: boolean };

export default function Mark({ size = 18, busy = false, alert = false }: Props) {
  const cls = ["mark", busy && "busy", alert && !busy && "alert"].filter(Boolean).join(" ");
  return (
    <svg className={cls} viewBox="0 0 120 120" width={size} height={size} aria-hidden="true">
      <line x1="36" y1="30" x2="62" y2="88" stroke="#f7f8f7" strokeWidth="12" strokeLinecap="round" />
      <circle className="mark-dot" cx="82" cy="86" r="8" fill="#6fcf9f" />
    </svg>
  );
}
