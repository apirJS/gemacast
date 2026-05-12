import { h } from '../utils';

export type ToastLevel = 'success' | 'error' | 'warning' | 'info';

const ICONS: Record<ToastLevel, string> = {
  success:
    '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M22 11.08V12a10 10 0 1 1-5.93-9.14"></path><polyline points="22 4 12 14.01 9 11.01"></polyline></svg>',
  error:
    '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="10"></circle><line x1="15" y1="9" x2="9" y2="15"></line><line x1="9" y1="9" x2="15" y2="15"></line></svg>',
  warning:
    '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z"></path><line x1="12" y1="9" x2="12" y2="13"></line><line x1="12" y1="17" x2="12.01" y2="17"></line></svg>',
  info: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="10"></circle><line x1="12" y1="16" x2="12" y2="12"></line><line x1="12" y1="8" x2="12.01" y2="8"></line></svg>',
};

export class ToastManager {
  private container: HTMLElement | null = null;
  private modal: HTMLDialogElement | null = null;
  private modalBody: HTMLElement | null = null;
  private activeErrorToast: HTMLElement | null = null;

  constructor() {
    if (typeof document === 'undefined') return;

    this.container = document.getElementById('toast-container');
    this.modal = document.getElementById(
      'error-log-modal',
    ) as HTMLDialogElement;
    this.modalBody = document.getElementById('error-log-modal-body');

    document
      .getElementById('error-log-modal-close')
      ?.addEventListener('click', () => {
        this.modal?.close();
      });

    this.modal?.addEventListener('click', (e) => {
      if (e.target === this.modal) this.modal?.close();
    });
  }

  public showSuccess(message: string) {
    this.createToast('success', message);
  }

  public showInfo(message: string) {
    this.createToast('info', message);
  }

  public showWarning(message: string) {
    this.createToast('warning', message);
  }

  public showError(message: string, fullLog?: string) {
    if (this.activeErrorToast) {
      this.removeToast(this.activeErrorToast);
    }
    this.activeErrorToast = this.createToast('error', message, fullLog);
  }

  public clearError() {
    if (this.activeErrorToast) {
      this.removeToast(this.activeErrorToast);
      this.activeErrorToast = null;
    }
  }

  private createToast(
    level: ToastLevel,
    message: string,
    fullLog?: string,
  ): HTMLElement {
    if (typeof document === 'undefined') return null as any;
    if (!this.container) return document.createElement('div');

    const iconWrap = h('div', { className: 'toast__icon' });
    iconWrap.innerHTML = ICONS[level];

    const contentWrap = h(
      'div',
      { className: 'toast__content' },
      h('span', { className: 'toast__message', textContent: message }),
    );

    if (level === 'error' && fullLog) {
      const detailsBtn = h('button', {
        className: 'toast__action',
        textContent: 'Click for details',
        onClick: () => {
          if (this.modal && this.modalBody) {
            this.modalBody.textContent = fullLog;
            this.modal.showModal();
          }
        },
      });
      contentWrap.appendChild(detailsBtn);
    }

    const toast = h(
      'div',
      { className: `toast toast--${level}`, role: 'alert' },
      iconWrap,
      contentWrap,
      h('button', {
        className: 'toast__close',
        ariaLabel: 'Close toast',
        onClick: () => {
          if (level === 'error' && this.activeErrorToast === toast) {
            this.activeErrorToast = null;
          }
          this.removeToast(toast);
        },
      }),
    );

    const closeBtn = toast.querySelector('.toast__close') as HTMLButtonElement;
    closeBtn.innerHTML =
      '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><line x1="18" y1="6" x2="6" y2="18"></line><line x1="6" y1="6" x2="18" y2="18"></line></svg>';

    this.container.appendChild(toast);

    if (level !== 'error') {
      setTimeout(() => this.removeToast(toast), 3000);
    }

    return toast;
  }

  private removeToast(toast: HTMLElement) {
    if (toast.classList.contains('toast--closing')) return;
    toast.classList.add('toast--closing');
    setTimeout(() => {
      if (toast.parentElement) toast.remove();
    }, 200);
  }
}

// Global singleton
export const toastManager = new ToastManager();
