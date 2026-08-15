export const IS_LINUX = typeof window !== 'undefined'
  && '__TAURI_INTERNALS__' in window
  && typeof navigator !== 'undefined'
  && /Linux/i.test(navigator.userAgent)

export const DEFAULT_PTT_SHORTCUT = IS_LINUX
  ? 'ControlLeft+AltLeft+Space'
  : 'ControlRight'

export const DEFAULT_HANDS_FREE_SHORTCUT = IS_LINUX
  ? 'Control+Alt+L'
  : 'AltRight'
