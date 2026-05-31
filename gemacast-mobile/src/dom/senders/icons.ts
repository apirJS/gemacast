import { DiscoveredSender } from '../../types';
import { h } from '../utils';

export function createIcon(isAdb: boolean) {
  const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  svg.setAttribute('viewBox', '0 0 24 24');
  svg.setAttribute('fill', 'currentColor');
  svg.setAttribute('width', '20');
  svg.setAttribute('height', '20');
  svg.setAttribute('class', 'sender-list__icon');
  svg.setAttribute('aria-hidden', 'true');

  if (isAdb) {
    svg.innerHTML =
      '<path d="M15 7v4h1v2h-3V5h2l-3-4-3 4h2v8H8v-2.07c.7-.37 1.2-1.08 1.2-1.93 0-1.21-.99-2.2-2.2-2.2-1.21 0-2.2.99-2.2 2.2 0 .85.5 1.56 1.2 1.93V13c0 1.11.89 2 2 2h3v3.05c-.71.37-1.2 1.08-1.2 1.95 0 1.21.99 2.2 2.2 2.2 1.21 0 2.2-.99 2.2-2.2 0-.87-.49-1.58-1.2-1.95V15h3c1.11 0 2-.89 2-2v-2h1V7h-4z"/>';
  } else {
    const path = document.createElementNS('http://www.w3.org/2000/svg', 'path');
    path.setAttribute(
      'd',
      'M21 2H3c-1.1 0-2 .9-2 2v12c0 1.1.9 2 2 2h7v2H8v2h8v-2h-2v-2h7c1.1 0 2-.9 2-2V4c0-1.1-.9-2-2-2zm0 14H3V4h18v12z',
    );
    svg.appendChild(path);
  }
  return svg;
}

export function createInfoBlock(sender: DiscoveredSender, isAdb: boolean) {
  return h(
    'div',
    { className: 'sender-list__info' },
    h('span', {
      className: 'sender-list__name',
      textContent: sender.deviceName,
    }),
    h(
      'div',
      { className: 'sender-list__ip-wrap' },
      h('span', {
        className: 'sender-list__ip',
        textContent: isAdb ? 'Direct USB Cable' : sender.addr.split(':')[0],
      }),
    ),
  );
}

export const chevronSvg = `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M6 9l6 6 6-6"/></svg>`;
export const refreshSvg = `<svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M1 4v6h6"/><path d="M3.51 15a9 9 0 1 0 2.13-9.36L1 10"/></svg>`;
