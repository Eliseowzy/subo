// 线性图标，24 网格、1.75 线宽、圆头圆角，颜色跟随 currentColor。
// 自己画的少量几何，不引图标库：离线包越小越好，也不用管许可证。

const P: Record<string, string> = {
  folder: "M3.5 7.5a2 2 0 0 1 2-2h4l2 2.5h7a2 2 0 0 1 2 2V17a2 2 0 0 1-2 2h-13a2 2 0 0 1-2-2z",
  folderOpen:
    "M3.5 17V7.5a2 2 0 0 1 2-2h4l2 2.5h6a2 2 0 0 1 2 2v1M3.5 17l2.2-5.6a1.5 1.5 0 0 1 1.4-.9H21l-2.4 7.2a1.5 1.5 0 0 1-1.4 1H5.5a2 2 0 0 1-2-1.7",
  file: "M14 3.5H7a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8.5zM14 3.5v5h5",
  tex: "M14 3.5H7a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8.5zM14 3.5v5h5M9.5 11.5l2.5 6M14.5 17.5h.01",
  bib: "M5 5.5a2 2 0 0 1 2-2h11.5v14H7a2 2 0 0 0-2 2zM5 19.5a2 2 0 0 0 2 2h11.5v-4M9 8h6M9 11h4",
  image: "M5 4.5h14a1.5 1.5 0 0 1 1.5 1.5v12a1.5 1.5 0 0 1-1.5 1.5H5A1.5 1.5 0 0 1 3.5 18V6A1.5 1.5 0 0 1 5 4.5zM3.5 16l4.5-4.5 4 4 2.5-2.5 6 6M15.5 9.5h.01",
  font: "M5 19 10.5 5h1L17 19M7.3 14h7.4M16.5 19h3",
  pdf: "M14 3.5H7a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8.5zM14 3.5v5h5M8.5 16.5h7M8.5 13h7",
  binary: "M14 3.5H7a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8.5zM14 3.5v5h5M10 12v4M14 12v4",
  chevron: "m9 6 6 6-6 6",
  chevronDown: "m6 9 6 6 6-6",
  back: "M19 12H5M11 6l-6 6 6 6",
  forward: "M5 12h14M13 6l6 6-6 6",
  plus: "M12 5v14M5 12h14",
  minus: "M5 12h14",
  grip: "M9 6h.01M15 6h.01M9 12h.01M15 12h.01M9 18h.01M15 18h.01",
  upload: "M12 15V4M7.5 8.5 12 4l4.5 4.5M4.5 15v3a2 2 0 0 0 2 2h11a2 2 0 0 0 2-2v-3",
  search: "M11 18a7 7 0 1 0 0-14 7 7 0 0 0 0 14zM20 20l-4-4",
  play: "M7 4.8v14.4a.8.8 0 0 0 1.2.7l11.3-7.2a.8.8 0 0 0 0-1.4L8.2 4.1a.8.8 0 0 0-1.2.7z",
  terminal: "M4.5 5h15a1.5 1.5 0 0 1 1.5 1.5v11a1.5 1.5 0 0 1-1.5 1.5h-15A1.5 1.5 0 0 1 3 17.5v-11A1.5 1.5 0 0 1 4.5 5zM7 10l3 2.5L7 15M12.5 15H17",
  reveal: "M3.5 7.5a2 2 0 0 1 2-2h4l2 2.5h7a2 2 0 0 1 2 2V17a2 2 0 0 1-2 2h-13a2 2 0 0 1-2-2zM12 11v5M9.5 13.5 12 11l2.5 2.5",
  pencil: "M15.5 5.5 18.5 8.5M4.5 19.5l1-4L16 5a2.1 2.1 0 0 1 3 3L8.5 18.5z",
  x: "M6 6l12 12M18 6 6 18",
  sun: "M12 16a4 4 0 1 0 0-8 4 4 0 0 0 0 8zM12 2.5v2M12 19.5v2M4.6 4.6 6 6M18 18l1.4 1.4M2.5 12h2M19.5 12h2M4.6 19.4 6 18M18 6l1.4-1.4",
  moon: "M20 14.5A8 8 0 1 1 9.5 4a6.5 6.5 0 0 0 10.5 10.5z",
  globe: "M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18zM3.5 9h17M3.5 15h17M12 3c-2.4 2.5-3.5 5.5-3.5 9s1.1 6.5 3.5 9M12 3c2.4 2.5 3.5 5.5 3.5 9s-1.1 6.5-3.5 9",
  monitor: "M4.5 4.5h15A1.5 1.5 0 0 1 21 6v9.5a1.5 1.5 0 0 1-1.5 1.5h-15A1.5 1.5 0 0 1 3 15.5V6a1.5 1.5 0 0 1 1.5-1.5zM8.5 20.5h7M12 17v3.5",
  alert: "M12 9v4M12 16.5h.01M10.3 4.3 2.8 17.5A2 2 0 0 0 4.5 20.5h15a2 2 0 0 0 1.7-3L13.7 4.3a2 2 0 0 0-3.4 0z",
  error: "M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18zM12 7.5v5M12 16h.01",
  check: "M5 12.5l4.5 4.5L19 7.5",
  info: "M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18zM12 11v5M12 8h.01",
  copy: "M9 9h10.5v10.5H9zM15 9V5.5a1 1 0 0 0-1-1H5.5a1 1 0 0 0-1 1V14a1 1 0 0 0 1 1H9",
  download: "M12 4v11M7.5 10.5 12 15l4.5-4.5M4.5 15v3a2 2 0 0 0 2 2h11a2 2 0 0 0 2-2v-3",
  fitWidth: "M4 4v16M20 4v16M7.5 12h9M10 9.5 7.5 12l2.5 2.5M14 9.5l2.5 2.5-2.5 2.5",
  fitPage: "M7 3.5h10a1.5 1.5 0 0 1 1.5 1.5v14a1.5 1.5 0 0 1-1.5 1.5H7A1.5 1.5 0 0 1 5.5 19V5A1.5 1.5 0 0 1 7 3.5zM12 7v10M9.5 9.5 12 7l2.5 2.5M9.5 14.5 12 17l2.5-2.5",
  locate: "M12 20a8 8 0 1 0 0-16 8 8 0 0 0 0 16zM12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6zM12 2v3M12 19v3M2 12h3M19 12h3",
  home: "M4 10.5 12 4l8 6.5V19a1.5 1.5 0 0 1-1.5 1.5H15V15h-6v5.5H5.5A1.5 1.5 0 0 1 4 19z",
  more: "M6 12h.01M12 12h.01M18 12h.01",
  trash: "M4.5 7h15M9.5 7V4.5h5V7M6.5 7l.8 12a1.5 1.5 0 0 0 1.5 1.5h6.4a1.5 1.5 0 0 0 1.5-1.5l.8-12",
  lock: "M6.5 10.5h11a1.5 1.5 0 0 1 1.5 1.5v7a1.5 1.5 0 0 1-1.5 1.5h-11A1.5 1.5 0 0 1 5 19v-7a1.5 1.5 0 0 1 1.5-1.5zM8 10.5V8a4 4 0 0 1 8 0v2.5",
  keyboard: "M4 6h16a1.5 1.5 0 0 1 1.5 1.5v9A1.5 1.5 0 0 1 20 18H4a1.5 1.5 0 0 1-1.5-1.5v-9A1.5 1.5 0 0 1 4 6zM7 10h.01M11 10h.01M15 10h.01M8 14h8",
  template:
    "M8.5 7V5.5a2 2 0 0 1 2-2H18a2 2 0 0 1 2 2V15a2 2 0 0 1-2 2h-1.5M6 7h8.5a2 2 0 0 1 2 2v9.5a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V9a2 2 0 0 1 2-2zM7.5 11h5.5M7.5 14.5h3.5",
  sparkle: "M12 3.5l1.9 5.1 5.1 1.9-5.1 1.9L12 17.5l-1.9-5.1L5 10.5l5.1-1.9zM18.5 15.5l.8 2.2 2.2.8-2.2.8-.8 2.2-.8-2.2-2.2-.8 2.2-.8z",
  sliders: "M4 7h9M17 7h3M4 17h3M11 17h9M15 4.5v5M9 14.5v5",
  package: "M12 3.5 20 8v8l-8 4.5L4 16V8zM4 8l8 4.5L20 8M12 12.5v8M8 5.8l8 4.4",
  speed: "M4.5 17a8 8 0 1 1 15 0M12 13l3.5-4.5M12 14.5a1.5 1.5 0 1 0 0-3 1.5 1.5 0 0 0 0 3z",
  refresh: "M19.5 12a7.5 7.5 0 1 1-2.2-5.3M19.5 4.5v4h-4",
  history: "M4.5 12a7.5 7.5 0 1 0 2.2-5.3M4.5 4.5v4h4M12 8v4.5l3 2",
  outline: "M5 6.5h.01M9 6.5h10M7.5 12h.01M11.5 12h7.5M7.5 17.5h.01M11.5 17.5h7.5",
  archive: "M4 5h16v4H4zM5.5 9v9.5a1.5 1.5 0 0 0 1.5 1.5h10a1.5 1.5 0 0 0 1.5-1.5V9M10 13h4",
  unarchive: "M4 5h16v4H4zM5.5 9v9.5a1.5 1.5 0 0 0 1.5 1.5h10a1.5 1.5 0 0 0 1.5-1.5V9M12 17.5v-5M9.5 14.5 12 12l2.5 2.5",
  zip: "M14 3.5H7a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8.5zM14 3.5v5h5M10.5 4v2M10.5 8v2M10.5 12v2M9.5 16h2v2.5h-2z",
};

export type IconName = keyof typeof P;

type Props = { name: IconName; size?: number; className?: string; title?: string };

export default function Icon({ name, size = 16, className, title }: Props) {
  // play/more 用实心更清楚
  const solid = name === "play";
  return (
    <svg
      className={className ? `icon ${className}` : "icon"}
      viewBox="0 0 24 24"
      width={size}
      height={size}
      fill={solid ? "currentColor" : "none"}
      stroke="currentColor"
      strokeWidth={name === "more" || name === "grip" ? 3 : 1.75}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden={title ? undefined : true}
      role={title ? "img" : undefined}
    >
      {title && <title>{title}</title>}
      <path d={P[name]} />
    </svg>
  );
}

/** 文件树、标签上用的类型图标 */
export function fileIcon(path: string): IconName {
  const p = path.toLowerCase();
  if (/\.(tex|sty|cls|ltx|dtx)$/.test(p)) return "tex";
  if (/\.(bib|bst|bbl)$/.test(p)) return "bib";
  if (/\.(png|jpe?g|gif|bmp|webp|svg|eps|tiff?)$/.test(p)) return "image";
  if (/\.(otf|ttf|ttc|woff2?|pfb|afm|tfm)$/.test(p)) return "font";
  if (p.endsWith(".pdf")) return "pdf";
  if (/\.(zip|gz|tgz|xz)$/.test(p)) return "zip";
  if (p.endsWith(".subocase")) return "package";
  return "file";
}
