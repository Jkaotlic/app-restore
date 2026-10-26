// Маленький помощник для DOM. Текст ставится только через textContent: данные из Apple
// в разметку не попадают. Цвета ставятся через CSSOM (CSP без 'unsafe-inline').

type Child = Node | string | null | undefined | false;
type Props = {
  class?: string;
  text?: string;
  style?: Partial<Record<"background" | "width", string>>;
  on?: Partial<Record<"click" | "input" | "keydown" | "paste" | "change" | "submit", (e: any) => void>>;
  attrs?: Record<string, string | boolean | undefined>;
};

export function h<K extends keyof HTMLElementTagNameMap>(tag: K, props: Props = {}, ...children: Child[]): HTMLElementTagNameMap[K] {
  const el = document.createElement(tag);
  if (props.class) el.className = props.class;
  if (props.text !== undefined) el.textContent = props.text;
  if (props.style) for (const [k, v] of Object.entries(props.style)) if (v !== undefined) el.style.setProperty(k, v);
  if (props.on) for (const [k, fn] of Object.entries(props.on)) if (fn) el.addEventListener(k, fn);
  if (props.attrs)
    for (const [k, v] of Object.entries(props.attrs)) {
      if (v === true) el.setAttribute(k, "");
      else if (typeof v === "string") el.setAttribute(k, v);
    }
  for (const c of children) if (c) el.append(c);
  return el;
}

export function phoneIcon(): SVGSVGElement {
  const ns = "http://www.w3.org/2000/svg";
  const svg = document.createElementNS(ns, "svg");
  svg.setAttribute("width", "34");
  svg.setAttribute("height", "56");
  svg.setAttribute("viewBox", "0 0 34 56");
  svg.setAttribute("aria-hidden", "true");
  const body = document.createElementNS(ns, "rect");
  for (const [k, v] of Object.entries({ x: "1.5", y: "1.5", width: "31", height: "53", rx: "7", fill: "none", stroke: "#13312F", "stroke-width": "2.5" }))
    body.setAttribute(k, v);
  const notch = document.createElementNS(ns, "rect");
  for (const [k, v] of Object.entries({ x: "12", y: "5", width: "10", height: "3", rx: "1.5", fill: "#13312F" })) notch.setAttribute(k, v);
  svg.append(body, notch);
  return svg;
}

/** iPad: шире, без выреза. */
export function ipadIcon(): SVGSVGElement {
  const ns = "http://www.w3.org/2000/svg";
  const svg = document.createElementNS(ns, "svg");
  svg.setAttribute("width", "46");
  svg.setAttribute("height", "56");
  svg.setAttribute("viewBox", "0 0 46 56");
  svg.setAttribute("aria-hidden", "true");
  const body = document.createElementNS(ns, "rect");
  for (const [k, v] of Object.entries({ x: "1.5", y: "1.5", width: "43", height: "53", rx: "5", fill: "none", stroke: "#13312F", "stroke-width": "2.5" }))
    body.setAttribute(k, v);
  const cam = document.createElementNS(ns, "circle");
  for (const [k, v] of Object.entries({ cx: "23", cy: "6", r: "1.5", fill: "#13312F" })) cam.setAttribute(k, v);
  svg.append(body, cam);
  return svg;
}
