import { useEffect, useRef } from "react";

// 首页左栏的符号画：一副活字字盘（case 的本义）。格子大小不一，每格一个 LaTeX / 印刷符号，
// 其中一格放着标志里的那个点。纯装饰，颜色压得很淡，不承载信息。
// 动效：鼠标附近的格子微微亮起；悬停按下去；点一格，那枚活字被拣起来再落回；
// 闲着时隔几秒自己拣一枚；键盘敲到盘里有的符号（\ { } $ & % # ^ _ ~），对应那格跳一下。
// 放点的那格会慢慢呼吸，点它就跳一下。系统开了「减少动态效果」就只留静态和悬停高亮。

/** 每行各格的宽度，加起来都是 400；参照老式字盘，常用字的格子大 */
const ROWS = [
  [50, 50, 50, 100, 50, 50, 50],
  [40, 40, 80, 40, 40, 80, 40, 40],
  [100, 50, 50, 50, 50, 100],
];
// i18n-ignore：字盘里的装饰符号，不是界面文案，不随语言变
const GLYPHS = [
  "§", "¶", "{", "\\", "}", "$", "&",
  "α", "∫", "永", "π", "∑", "", "√", "%",
  "排", "#", "^", "_", "~", "字",
];
/** 放标志那个点的格子（GLYPHS 里留空的那个） */
const DOT = GLYPHS.indexOf("");
const ROW_H = 50;

/** 各格中心，算鼠标远近用 */
const CENTERS: [number, number][] = ROWS.flatMap((row, r) => {
  let x = 0;
  return row.map((w) => {
    const c: [number, number] = [x + w / 2, r * ROW_H + ROW_H / 2];
    x += w;
    return c;
  });
});
/** 亮起的半径（viewBox 单位） */
const REACH = 90;

const reduced = () => window.matchMedia("(prefers-reduced-motion: reduce)").matches;

export default function TypeCase() {
  const svgRef = useRef<SVGSVGElement>(null);
  const cells = useRef<(SVGGElement | null)[]>([]);
  const hovering = useRef(false);

  // 拣起一枚：重放 pick 动画（点那格是 hop）
  const pick = (i: number) => {
    const g = cells.current[i];
    if (!g || reduced()) return;
    g.classList.remove("pick");
    void g.getBoundingClientRect(); // 让同一格连点也能重新播
    g.classList.add("pick");
  };

  // 闲着时自己拣字；页面看不见、鼠标在盘上、减少动效时都不动
  useEffect(() => {
    let timer = 0;
    const loop = () => {
      timer = window.setTimeout(
        () => {
          if (!document.hidden && !hovering.current) {
            let i = Math.floor(Math.random() * GLYPHS.length);
            if (i === DOT) i = (i + 1) % GLYPHS.length;
            pick(i);
          }
          loop();
        },
        3500 + Math.random() * 4000,
      );
    };
    loop();
    return () => clearTimeout(timer);
  }, []);

  // 键盘敲到盘里有的符号就让那格跳一下；在输入框里打字、带修饰键时不管
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.metaKey || e.ctrlKey || e.altKey || e.repeat) return;
      const el = e.target as HTMLElement | null;
      if (el && (el.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(el.tagName))) return;
      const i = GLYPHS.indexOf(e.key);
      if (i >= 0 && i !== DOT) pick(i);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  // 鼠标附近的格子按远近亮起，写到各格的 --near（0~1）
  const onMove = (e: React.PointerEvent<SVGSVGElement>) => {
    const svg = svgRef.current;
    const m = svg?.getScreenCTM();
    if (!svg || !m) return;
    const p = new DOMPoint(e.clientX, e.clientY).matrixTransform(m.inverse());
    CENTERS.forEach(([cx, cy], i) => {
      const d = Math.hypot(cx - p.x, cy - p.y);
      cells.current[i]?.style.setProperty("--near", Math.max(0, 1 - d / REACH).toFixed(2));
    });
  };
  const onLeave = () => {
    hovering.current = false;
    cells.current.forEach((g) => g?.style.setProperty("--near", "0"));
  };

  let k = 0;
  return (
    <svg
      ref={svgRef}
      className="type-case"
      viewBox={`-8 -8 416 ${ROWS.length * ROW_H + 16}`}
      aria-hidden="true"
      onPointerEnter={() => (hovering.current = true)}
      onPointerMove={onMove}
      onPointerLeave={onLeave}
    >
      {/* 字盘外框：整副盘画全，不再往下淡出成半截 */}
      <rect className="tc-frame" x="-5" y="-5" width="410" height={ROWS.length * ROW_H + 10} rx="9" />
      {ROWS.map((row, r) => {
        let x = 0;
        return row.map((w) => {
          const i = k++;
          const cx = x + w / 2;
          const cy = r * ROW_H + ROW_H / 2;
          const cell = (
            <g
              key={i}
              ref={(el) => (cells.current[i] = el)}
              className={i === DOT ? "tc-cell tc-dot-cell" : "tc-cell"}
              style={{ animationDelay: `${(i % 9) * 45 + r * 60}ms` }}
              onClick={() => pick(i)}
              onAnimationEnd={(e) => {
                if (e.animationName.startsWith("tc-pick") || e.animationName === "tc-hop")
                  e.currentTarget.classList.remove("pick");
              }}
            >
              <rect x={x + 3} y={r * ROW_H + 3} width={w - 6} height={ROW_H - 6} rx="5" />
              {i === DOT ? (
                <circle className="tc-dot" cx={cx} cy={cy} r="6.5" />
              ) : (
                <text x={cx} y={cy} fontSize={Math.min(w, ROW_H) * 0.46}>
                  {GLYPHS[i]}
                </text>
              )}
            </g>
          );
          x += w;
          return cell;
        });
      })}
    </svg>
  );
}
