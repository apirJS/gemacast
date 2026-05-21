import { App } from '../../App';
import { AppState, AudioSource } from '../../types';
import { h } from '../utils';
import { chevronSvg, refreshSvg } from './icons';
import { sourceIcon, sourceLabel, sourcesEqual, buildSourceOptions } from './source';

/**
 * Module-level state that persists across subscriber re-renders.
 * The subscriber destroys and rebuilds the entire sender list on every
 * `setState()` call (latency updates, network checks, etc.), so local
 * DOM state like "is the dropdown open?" must live here.
 */
export let dropdownOpen = false;
export let dropdownScrollTop = 0;
export let dropdownSearchQuery = '';
export let searchInputFocused = false;
export let searchSelectionStart = 0;
export let searchSelectionEnd = 0;

/** Cleanup reference so we can remove the document listener before DOM teardown. */
let activeOutsideClickHandler: ((e: MouseEvent) => void) | null = null;
let activeEscapeHandler: ((e: KeyboardEvent) => void) | null = null;

export function teardownDropdownListeners() {
  if (activeOutsideClickHandler) {
    document.removeEventListener('click', activeOutsideClickHandler, true);
    activeOutsideClickHandler = null;
  }
  if (activeEscapeHandler) {
    document.removeEventListener('keydown', activeEscapeHandler);
    activeEscapeHandler = null;
  }
}

export function resetDropdownState() {
  dropdownOpen = false;
  dropdownScrollTop = 0;
  dropdownSearchQuery = '';
  searchInputFocused = false;
}

export function setDropdownOpen(open: boolean) {
  dropdownOpen = open;
}

export function createProcessSelect(
  app: App,
  state: AppState,
  currentSource: AudioSource,
  onSourceChange: (source: AudioSource) => void,
) {
  const caps = state.senderCapabilities;
  const onlyDesktop =
    !caps?.supportsProcessCapture &&
    state.audioSources.length <= 1 &&
    state.processList.length === 0;

  const allSources = buildSourceOptions(state.audioSources, state.processList);

  const isOpen = dropdownOpen && !onlyDesktop;

  const container = h('div', {
    className: `process-select${isOpen ? ' process-select--open' : ''}`,
  });

  container.appendChild(
    h('span', { className: 'process-select__label', textContent: 'Source:' }),
  );

  // Trigger button
  const trigger = h(
    'button',
    {
      className: 'process-select__trigger',
      type: 'button',
      disabled: onlyDesktop,
      ariaLabel: 'Select audio source',
      ariaHasPopup: 'listbox',
      ariaExpanded: isOpen ? 'true' : 'false',
    },
    (() => {
      const iconEl = h('span', { className: 'process-select__trigger-icon' });
      iconEl.innerHTML = sourceIcon(currentSource);
      return iconEl;
    })(),
    h('span', {
      className: 'process-select__trigger-label',
      textContent: sourceLabel(currentSource),
    }),
  );

  const chevronEl = h('span', { className: 'process-select__trigger-chevron' });
  chevronEl.innerHTML = chevronSvg;
  trigger.appendChild(chevronEl);
  container.appendChild(trigger);

  // Dropdown panel
  const dropdown = h('div', {
    className: 'process-select__dropdown',
    role: 'listbox',
    ariaLabel: 'Audio sources',
  });
  
  // Search bar
  const searchInput = h('input', {
    className: 'process-select__search',
    type: 'text',
    placeholder: 'Search process...',
    value: dropdownSearchQuery,
  }) as HTMLInputElement;
  
  dropdown.appendChild(searchInput);

  const optionsList = h('div', { className: 'process-select__options' });

  // Track scroll position at module level
  optionsList.addEventListener('scroll', () => {
    dropdownScrollTop = optionsList.scrollTop;
  });

  for (const source of allSources) {
    const isActive = sourcesEqual(source, currentSource);
    const option = h(
      'div',
      {
        className: `process-select__option${isActive ? ' process-select__option--active' : ''}`,
        role: 'option',
        ariaSelected: isActive ? 'true' : 'false',
        tabIndex: 0,
        onClick: async (e) => {
          e.stopPropagation();
          const prevSource = currentSource;
          onSourceChange(source);
          closeDropdown();
          
          const result = await app.connection.changeAudioSource(source);
          
          if (!result.ok) {
            console.error('Failed to change audio source', result.error);
            onSourceChange(prevSource);
            app.stateHandler.setState({}); // Revert UI
          }
        },
      },
      (() => {
        const iconEl = h('span', {
          className: `process-select__option-icon${source.type === 'process' && source.hasAudioSession ? ' process-select__option-icon--audio-active' : ''}`,
        });
        iconEl.innerHTML = sourceIcon(source);
        return iconEl;
      })(),
      h('span', {
        className: 'process-select__option-label',
        textContent: sourceLabel(source),
      }),
    );
    optionsList.appendChild(option);
  }
  
  function filterOptions(query: string) {
    const lowerQuery = query.toLowerCase();
    Array.from(optionsList.children).forEach((child) => {
      const option = child as HTMLElement;
      if (option.classList.contains('process-select__option')) {
        const label = option.querySelector('.process-select__option-label')?.textContent?.toLowerCase() || '';
        if (label.includes(lowerQuery)) {
          option.style.display = '';
        } else {
          option.style.display = 'none';
        }
      }
    });
  }
  
  if (dropdownSearchQuery) {
    filterOptions(dropdownSearchQuery);
  }

  searchInput.addEventListener('input', (e) => {
    const target = e.target as HTMLInputElement;
    dropdownSearchQuery = target.value;
    filterOptions(dropdownSearchQuery);
  });
  
  searchInput.addEventListener('focus', () => {
    searchInputFocused = true;
  });
  
  searchInput.addEventListener('blur', () => {
    searchInputFocused = false;
  });
  
  searchInput.addEventListener('keyup', (e) => {
    const target = e.target as HTMLInputElement;
    searchSelectionStart = target.selectionStart || 0;
    searchSelectionEnd = target.selectionEnd || 0;
  });
  
  searchInput.addEventListener('click', (e) => {
    e.stopPropagation();
  });
  
  searchInput.addEventListener('mousedown', (e) => {
    e.stopPropagation();
  });

  dropdown.appendChild(optionsList);

  // Refresh button
  const refreshBtn = h(
    'button',
    {
      className: 'process-select__refresh',
      type: 'button',
      ariaLabel: 'Refresh process list',
      onClick: async (e) => {
        e.stopPropagation();
        refreshBtn.classList.add('process-select__refresh--loading');
        const sender = state.connectedSender;
        if (sender) {
          await app.connection.fetchProcessList(sender);
        }
        // The state update will trigger a full re-render.
        // dropdownOpen remains true so the dropdown stays open.
      },
    },
  );
  const refreshIconEl = h('span', { className: 'process-select__refresh-icon' });
  refreshIconEl.innerHTML = refreshSvg;
  refreshBtn.appendChild(refreshIconEl);
  refreshBtn.appendChild(document.createTextNode(' Refresh'));
  dropdown.appendChild(refreshBtn);

  container.appendChild(dropdown);

  // Open/close logic — mutates module-level `dropdownOpen`
  function openDropdown() {
    dropdownOpen = true;
    container.classList.add('process-select--open');
    trigger.setAttribute('aria-expanded', 'true');
    installDocumentListeners();
  }

  function closeDropdown() {
    dropdownOpen = false;
    dropdownScrollTop = 0;
    dropdownSearchQuery = '';
    searchInputFocused = false;
    container.classList.remove('process-select--open');
    trigger.setAttribute('aria-expanded', 'false');
    teardownDropdownListeners();
  }

  function installDocumentListeners() {
    // Clean up any stale listeners first
    teardownDropdownListeners();

    const outsideHandler = (e: MouseEvent) => {
      const target = e.target as Node;
      // Use closest() for robustness: the container reference may be stale
      // after a DOM rebuild triggered by background state updates.
      if (!(target instanceof Element && target.closest('.process-select'))) {
        closeDropdown();
      }
    };

    const escapeHandler = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        closeDropdown();
        trigger.focus();
      }
    };

    activeOutsideClickHandler = outsideHandler;
    activeEscapeHandler = escapeHandler;

    // Defer registration to avoid catching the current click event
    requestAnimationFrame(() => {
      document.addEventListener('click', outsideHandler, true);
      document.addEventListener('keydown', escapeHandler);
    });
  }

  trigger.addEventListener('click', (e) => {
    e.stopPropagation();
    if (dropdownOpen) {
      closeDropdown();
    } else {
      openDropdown();
    }
  });

  // If the dropdown should be open on this render, install document listeners
  if (isOpen) {
    installDocumentListeners();
  }

  return { el: container, optionsList, searchInput };
}
