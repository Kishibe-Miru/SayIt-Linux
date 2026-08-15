import { IS_LINUX } from '@/lib/platform'

export type OverlayWaveTheme = 'black-white' | 'black-blue' | 'black-rainbow'

// 快捷键映射已收敛到 @/lib/shortcutKeys（单一数据源），这里透传导出以保持既有引用不变。
export {
  displayAccelerator,
  getSingleKeyDisplay,
  resolveSingleKeyShortcut,
} from '@/lib/shortcutKeys'

export function cleanMicLabel(label: string): string {
  return label.replace(/\s*\([0-9a-f]{4}:[0-9a-f]{4}\)\s*$/i, '').trim()
}

export function eventToAccelerator(event: KeyboardEvent): string | null {
  const parts: string[] = []
  if (IS_LINUX) {
    if (event.ctrlKey) parts.push('Control')
    if (event.metaKey) parts.push('Super')
  } else if (event.ctrlKey || event.metaKey) {
    parts.push('CommandOrControl')
  }
  if (event.altKey) parts.push('Alt')
  if (event.shiftKey) parts.push('Shift')

  const key = event.key
  if (['Control', 'Alt', 'Shift', 'Meta'].includes(key)) return null

  const keyMap: Record<string, string> = {
    ' ': 'Space',
    ArrowUp: 'Up',
    ArrowDown: 'Down',
    ArrowLeft: 'Left',
    ArrowRight: 'Right',
    Escape: 'Escape',
    Enter: 'Return',
    Backspace: 'Backspace',
    Delete: 'Delete',
    Tab: 'Tab',
  }

  const mapped = keyMap[key] || (key.length === 1 ? key.toUpperCase() : key)
  parts.push(mapped)
  return parts.length >= 2 ? parts.join('+') : null
}
