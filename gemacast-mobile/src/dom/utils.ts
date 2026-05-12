export function fmt(ms: number | null): string {
  return ms !== null ? `${ms.toFixed(1)} ms` : '-- ms';
}

export function h<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  props: Partial<HTMLElementTagNameMap[K]> & {
    onClick?: (e: MouseEvent) => void;
    onChange?: (e: Event) => void;
    dataset?: Record<string, string>;
  } = {},
  ...children: (string | Node | null | undefined | boolean)[]
): HTMLElementTagNameMap[K] {
  const el = document.createElement(tag);

  for (const [key, value] of Object.entries(props)) {
    if (value === undefined || value === null) continue;
    if (key === 'onClick') el.addEventListener('click', value as any);
    else if (key === 'onChange') el.addEventListener('change', value as any);
    else if (key === 'dataset') Object.assign(el.dataset, value);
    else if (key === 'className') el.className = value as string;
    else if (key in el) (el as any)[key] = value;
    else el.setAttribute(key, String(value));
  }

  for (const child of children) {
    if (typeof child === 'string') el.appendChild(document.createTextNode(child));
    else if (child && typeof child !== 'boolean') el.appendChild(child as Node);
  }

  return el;
}
