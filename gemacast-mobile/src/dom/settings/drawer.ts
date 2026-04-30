export function initDrawer() {
  const drawer = document.getElementById(
    'settings-drawer',
  ) as HTMLDialogElement;
  const openBtn = document.getElementById(
    'settings-open-btn',
  ) as HTMLButtonElement;
  const closeBtn = document.getElementById(
    'settings-close-btn',
  ) as HTMLButtonElement;

  openBtn.addEventListener('click', () => drawer.showModal());
  closeBtn.addEventListener('click', () => drawer.close());
  drawer.addEventListener('click', (e) => {
    if (e.target === drawer) drawer.close();
  });
}
