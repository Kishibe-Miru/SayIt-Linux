/**
 * 快捷键定义与 PTT 物理组合键工具。
 *
 * PTT 设置保存 DOM `KeyboardEvent.code`，旧单键保持 `ShiftRight` 等原格式，
 * 组合键按固定顺序保存为 `ControlLeft+MetaLeft`、`ControlLeft+KeyK`。
 *
 * ⚠️ Rust 端 `client/src-tauri/src/keyboard/mod.rs` 的按键表无法直接引用 TS；
 * 新增或修改 PTT code 时必须同步 Rust 映射。
 */

import { t, type TranslationKey } from '@/i18n'
import { IS_LINUX } from '@/lib/platform'

export interface SingleKeyDef {
  /** 存入设置、也等于 DOM KeyboardEvent.code */
  setting: string
  /** Windows 虚拟键码（供 webview 回退补发事件用） */
  vk: number
  /** 中文显示名 */
  label: string
}

// i18n-allow-start: Windows VK 源标签；展示层由 getSingleKeyDisplay 翻译覆盖
export const SINGLE_KEYS: SingleKeyDef[] = [
  // 修饰键（左右分开）
  { setting: 'AltLeft', vk: 0xa4, label: '左 Alt' },
  { setting: 'AltRight', vk: 0xa5, label: '右 Alt' },
  { setting: 'ControlLeft', vk: 0xa2, label: '左 Ctrl' },
  { setting: 'ControlRight', vk: 0xa3, label: '右 Ctrl' },
  { setting: 'ShiftLeft', vk: 0xa0, label: '左 Shift' },
  { setting: 'ShiftRight', vk: 0xa1, label: '右 Shift' },
  // 常见低冲突键
  { setting: 'CapsLock', vk: 0x14, label: 'Caps Lock' },
  { setting: 'Space', vk: 0x20, label: '空格' },
  { setting: 'ContextMenu', vk: 0x5d, label: '菜单键' },
  { setting: 'Pause', vk: 0x13, label: 'Pause' },
  { setting: 'ScrollLock', vk: 0x91, label: 'ScrollLock' },
  { setting: 'Insert', vk: 0x2d, label: 'Insert' },
  // 鼠标侧键（原始 XBUTTON，由 Rust 低级鼠标钩子处理）
  { setting: 'XButton1', vk: 0x05, label: '鼠标侧键1（后退）' },
  { setting: 'XButton2', vk: 0x06, label: '鼠标侧键2（前进）' },
  // 鼠标中键（VK_MBUTTON，由 Rust 低级鼠标钩子处理）
  { setting: 'MButton', vk: 0x04, label: '鼠标中键' },
  // 浏览器后退/前进键（罗技等改键鼠标常把侧键映射成这个，由键盘钩子处理）
  { setting: 'BrowserBack', vk: 0xa6, label: '鼠标侧键（后退键）' },
  { setting: 'BrowserForward', vk: 0xa7, label: '鼠标侧键（前进键）' },
  // 功能键
  ...Array.from({ length: 12 }, (_, index) => ({
    setting: `F${index + 1}`,
    vk: 0x70 + index,
    label: `F${index + 1}`,
  })),
]
// i18n-allow-end

/** setting → 虚拟键码（旧单键与免提回退使用） */
export const SETTING_TO_VK: Record<string, number> = Object.fromEntries(
  SINGLE_KEYS.map((key) => [key.setting, key.vk]),
)

const SINGLE_KEY_DISPLAY: Record<string, string> = Object.fromEntries(
  SINGLE_KEYS.map((key) => [key.setting, key.label]),
)

/** PTT 修饰键固定排序；左右位置会保留。 */
export const PTT_MODIFIER_CODES = [
  'ControlLeft',
  'ControlRight',
  'MetaLeft',
  'MetaRight',
  'AltLeft',
  'AltRight',
  'ShiftLeft',
  'ShiftRight',
] as const

const PTT_MODIFIER_SET = new Set<string>(PTT_MODIFIER_CODES)
const PTT_MOUSE_CODES = new Set(['XButton1', 'XButton2', 'MButton'])

/**
 * 不允许出现在「按住说话」里的按键。
 *
 * Shift 与 Windows 的辅助功能快捷方式直接冲突，而冲突的触发条件恰好就是 PTT 的
 * 使用方式：
 *   · 长按右 Shift 约 8 秒 → 筛选键。它一旦弹出，我们收不到那次「松开」，
 *     录音不会随手指抬起而结束（这是之前只给警告、用户仍然踩到的那个问题）。
 *   · 连按 Shift 五次 → 粘滞键。短句口述本来就是一串快速的按下松开。
 * PTT 必须被按住数秒、还经常被连续短按，两条正好全中。
 *
 * 所以 Shift 不做成"可以选但给个警告"——警告只能让用户在出问题之后回来读一遍，
 * 挡不住任何人。左右都禁：单键和组合成员都算，`Ctrl+右Shift` 一样会触发筛选键。
 *
 * **只约束 PTT。** 免提、预设切换那类"按一下"的快捷键（走 accelerator 那条校验）
 * 照旧可以用 Shift —— 它们不长按也不连按，两个触发条件都碰不到。
 */
const PTT_FORBIDDEN_CODES = new Set(['ShiftLeft', 'ShiftRight'])

/** 这个按键能不能用于「按住说话」。 */
export function isPTTForbiddenCode(code: string): boolean {
  return PTT_FORBIDDEN_CODES.has(code)
}

// i18n-allow-start: Windows VK 源标签；展示层由 getSingleKeyDisplay 翻译覆盖
const PTT_EXTRA_KEY_DEFS: SingleKeyDef[] = [
  { setting: 'MetaLeft', vk: 0x5b, label: '左 Win' },
  { setting: 'MetaRight', vk: 0x5c, label: '右 Win' },
  ...Array.from({ length: 26 }, (_, index) => ({
    setting: `Key${String.fromCharCode(65 + index)}`,
    vk: 0x41 + index,
    label: String.fromCharCode(65 + index),
  })),
  ...Array.from({ length: 10 }, (_, index) => ({
    setting: `Digit${index}`,
    vk: 0x30 + index,
    label: String(index),
  })),
  { setting: 'Escape', vk: 0x1b, label: 'Esc' },
  { setting: 'Tab', vk: 0x09, label: 'Tab' },
  { setting: 'Enter', vk: 0x0d, label: 'Enter' },
  { setting: 'Backspace', vk: 0x08, label: 'Backspace' },
  { setting: 'Delete', vk: 0x2e, label: 'Delete' },
  { setting: 'ArrowUp', vk: 0x26, label: '↑' },
  { setting: 'ArrowDown', vk: 0x28, label: '↓' },
  { setting: 'ArrowLeft', vk: 0x25, label: '←' },
  { setting: 'ArrowRight', vk: 0x27, label: '→' },
  { setting: 'Home', vk: 0x24, label: 'Home' },
  { setting: 'End', vk: 0x23, label: 'End' },
  { setting: 'PageUp', vk: 0x21, label: 'Page Up' },
  { setting: 'PageDown', vk: 0x22, label: 'Page Down' },
]
// i18n-allow-end

/** PTT 成员 code → Windows 虚拟键码。 */
export const PTT_CODE_TO_VK: Record<string, number> = {
  ...SETTING_TO_VK,
  ...Object.fromEntries(PTT_EXTRA_KEY_DEFS.map((key) => [key.setting, key.vk])),
}

const PTT_CODE_DISPLAY: Record<string, string> = {
  ...SINGLE_KEY_DISPLAY,
  ...Object.fromEntries(PTT_EXTRA_KEY_DEFS.map((key) => [key.setting, key.label])),
}

/** 是否为受支持的旧单键设置。 */
export function isSingleKeySetting(setting: string): boolean {
  return setting in SETTING_TO_VK
}

/** 若 DOM code 对应一个受支持的旧单键，返回其 setting（= code）。 */
export function resolveSingleKeyShortcut(code: string): string | undefined {
  return isSingleKeySetting(code) ? code : undefined
}

/** setting → DOM code（恒等，未知返回空串）。 */
export function settingToCode(setting: string): string {
  return isSingleKeySetting(setting) ? setting : ''
}

/**
 * 需要翻译的键名。
 *
 * 只列**含自然语言**的那些：`左 Alt` 在英文里是 `Left Alt`，`空格` 是 `Space`。
 * `F1`、`Tab`、`Esc`、`Caps Lock`、字母数字、方向键这些本来就是语言中立的，
 * 继续用上面表里的 `label`，不进 locale 文件 —— 翻译一份和原文一样的东西，
 * 只会多一处将来会不同步的地方。
 *
 * ⚠️ 这里刻意**不改** `SINGLE_KEYS` 的 `label` 字段：那张表还同时供 `vk` 映射用，
 * 动它的形状会牵连 webview 回退补发按键的逻辑。这里只加一层展示期的覆盖。
 */
const TRANSLATED_KEY_NAMES: Record<string, TranslationKey> = {
  AltLeft: 'keyName.AltLeft',
  AltRight: 'keyName.AltRight',
  ControlLeft: 'keyName.ControlLeft',
  ControlRight: 'keyName.ControlRight',
  ShiftLeft: 'keyName.ShiftLeft',
  ShiftRight: 'keyName.ShiftRight',
  MetaLeft: 'keyName.MetaLeft',
  MetaRight: 'keyName.MetaRight',
  Space: 'keyName.Space',
  ContextMenu: 'keyName.ContextMenu',
  XButton1: 'keyName.XButton1',
  XButton2: 'keyName.XButton2',
  MButton: 'keyName.MButton',
  BrowserBack: 'keyName.BrowserBack',
  BrowserForward: 'keyName.BrowserForward',
}

/** 单键显示名（未知原样返回）。 */
export function getSingleKeyDisplay(value: string): string {
  const key = TRANSLATED_KEY_NAMES[value]
  if (key) return t(key)
  return SINGLE_KEY_DISPLAY[value] || value
}

/** Tauri accelerator 的 Windows 显示名，用于逐个渲染键帽。 */
export function displayAccelerator(accelerator: string): string[] {
  const displayNames: Record<string, string> = {
    CommandOrControl: 'Ctrl',
    Control: 'Ctrl',
    Ctrl: 'Ctrl',
    Alt: 'Alt',
    Shift: 'Shift',
    Space: 'Space',
    Return: 'Enter',
  }
  return accelerator.split('+')
    .map((part) => part.trim())
    .filter(Boolean)
    .map((part) => displayNames[part] || part)
}

/** 单键或 accelerator 的统一 Windows 显示标签。 */
export function displayShortcut(shortcut: string): string[] {
  return isSingleKeySetting(shortcut)
    ? [getSingleKeyDisplay(shortcut)]
    : displayAccelerator(shortcut)
}

/** 拆分 PTT 设置；不在这里静默丢弃未知成员，交给校验器给出明确错误。 */
export function parsePTTShortcut(setting: string): string[] {
  if (!setting.trim()) return []
  return setting.split('+').map((part) => part.trim()).filter(Boolean)
}

/** 将 PTT 成员按 Ctrl、Win、Alt、Shift、普通键顺序规范化。 */
export function canonicalizePTTShortcut(settingOrCodes: string | Iterable<string>): string {
  const codes = typeof settingOrCodes === 'string'
    ? parsePTTShortcut(settingOrCodes)
    : Array.from(settingOrCodes)
  const uniqueCodes = [...new Set(codes)]
  const modifierOrder = new Map<string, number>(
    PTT_MODIFIER_CODES.map((code, index) => [code, index]),
  )
  return uniqueCodes.sort((left, right) => {
    const leftOrder = modifierOrder.get(left) ?? PTT_MODIFIER_CODES.length
    const rightOrder = modifierOrder.get(right) ?? PTT_MODIFIER_CODES.length
    return leftOrder - rightOrder || left.localeCompare(right)
  }).join('+')
}

/** PTT 设置的按键标签，用于逐个渲染键帽。 */
export function displayPTTShortcut(setting: string): string[] {
  return parsePTTShortcut(canonicalizePTTShortcut(setting)).map((code) => {
    const key = TRANSLATED_KEY_NAMES[code]
    return key ? t(key) : PTT_CODE_DISPLAY[code] || code
  })
}

export function isPTTModifierCode(code: string): boolean {
  return PTT_MODIFIER_SET.has(code)
}

export function pttShortcutHasModifier(setting: string): boolean {
  return parsePTTShortcut(setting).some(isPTTModifierCode)
}

/**
 * 已保存的按住说话配置需要提醒用户的地方。
 *
 * 目前只有一种：老用户在 Shift 还能选的时候绑上了它。
 *
 * 为什么不在升级时自动改掉：那是在用户不知情的情况下换掉他的说话键，比留着更糟。
 * 也不在这里直接停掉它——那等于升级后按住说话突然没反应，而用户未必会想到来设置页。
 * 所以旧绑定继续生效，只是在设置里常驻一行提示让他换。新绑定由
 * getPTTShortcutValidationError 硬拦。
 */
export function getPTTShortcutWarning(setting: string): string | null {
  return parsePTTShortcut(canonicalizePTTShortcut(setting)).some(isPTTForbiddenCode)
    ? t('shortcut.warning.shiftNoLongerSupported')
    : null
}

function normalizedAcceleratorParts(accelerator: string): string[] {
  const aliases: Record<string, string> = {
    Ctrl: 'Control',
    CommandOrControl: 'Control',
    Super: 'Meta',
    Win: 'Meta',
    Windows: 'Meta',
    Return: 'Enter',
    Up: 'ArrowUp',
    Down: 'ArrowDown',
    Left: 'ArrowLeft',
    Right: 'ArrowRight',
  }
  return accelerator.split('+')
    .map((part) => part.trim())
    .filter(Boolean)
    .map((part) => aliases[part] || part)
}

/**
 * 通用 accelerator 的 Windows 保留组合校验。
 *
 * PTT 使用物理 code，有自己的完整校验；免提与预设切换使用 Tauri accelerator，
 * 也必须在试注册前挡住系统安全界面、窗口切换等无法可靠覆盖的组合。
 */
export function getAcceleratorShortcutValidationError(accelerator: string): string | null {
  const parts = normalizedAcceleratorParts(accelerator)
  const has = (key: string) => parts.includes(key)
  const mainKeys = parts.filter((part) => !['Control', 'Meta', 'Alt', 'Shift'].includes(part))
  const mainKey = mainKeys[0]

  if (has('Control') && has('Alt') && mainKey === 'Delete') {
    return t('shortcut.error.reservedCtrlAltDel')
  }
  if (!IS_LINUX && has('Meta') && mainKey) {
    return t('shortcut.error.reservedWindows')
  }
  const reservedAltMainKeys = IS_LINUX
    ? ['F4', 'Tab', 'Escape']
    : ['F4', 'Tab', 'Escape', 'Space']
  if (has('Alt')
    && (!IS_LINUX || (!has('Control') && !has('Meta')))
    && reservedAltMainKeys.includes(mainKey)) {
    return t('shortcut.error.reservedAlt')
  }
  if (has('Control') && mainKey === 'Escape') {
    return t('shortcut.error.reservedCtrlEsc')
  }
  return null
}

export interface PTTValidationOptions {
  /**
   * 是否放行「历史上曾经能选、现在已禁用」的按键（目前只有 Shift）。
   *
   * 读取**已保存的配置**时必须传 true。否则老用户的 Shift 绑定会被判成非法，
   * 而几个调用点碰到非法值的做法是回落到默认键 —— 用户一个设置都没改，
   * 说话键却自己变了，这比让他继续用 Shift 更糟。
   *
   * 校验**用户新输入**时保持 false（默认），这样 Shift 绑不上去。
   */
  allowLegacyReservedKeys?: boolean
}

/** 返回本地化的错误提示；null 表示可保存。 */
export function getPTTShortcutValidationError(
  setting: string,
  options: PTTValidationOptions = {},
): string | null {
  const rawCodes = setting.split('+').map((part) => part.trim())
  const codes = rawCodes.filter(Boolean)
  if (codes.length === 0) return t('shortcut.error.empty')
  if (rawCodes.length !== codes.length || new Set(codes).size !== codes.length) {
    return t('shortcut.error.invalidFormat')
  }

  const unsupported = codes.find((code) => !(code in PTT_CODE_TO_VK))
  if (unsupported) return t('shortcut.error.unsupportedKey', { key: unsupported })

  // 排在单键/组合的分支之前：Shift 无论单独用还是当组合成员都不行
  if (!options.allowLegacyReservedKeys && codes.some(isPTTForbiddenCode)) {
    return t('shortcut.error.shiftReserved')
  }

  if (codes.length === 1) {
    const code = codes[0]
    if (IS_LINUX) {
      return t('shortcut.error.linuxNeedsCombination')
    }
    if (code === 'MetaLeft' || code === 'MetaRight') {
      return t('shortcut.error.metaAlone')
    }
    if (!isSingleKeySetting(code)) {
      return t('shortcut.error.plainKeyAlone')
    }
    return null
  }

  if (codes.some((code) => PTT_MOUSE_CODES.has(code))) {
    return t('shortcut.error.mouseCombo')
  }

  const modifiers = codes.filter(isPTTModifierCode)
  const mainKeys = codes.filter((code) => !isPTTModifierCode(code))
  const modifierFamilies = modifiers.map((code) => code.replace(/(?:Left|Right)$/, ''))
  if (new Set(modifierFamilies).size !== modifierFamilies.length) {
    return t('shortcut.error.sameModifierFamily')
  }
  if (mainKeys.length > 1) return t('shortcut.error.tooManyMainKeys')
  if (mainKeys.length === 1 && modifiers.length === 0) {
    return t('shortcut.error.needModifier')
  }
  if (IS_LINUX && mainKeys.length === 0) {
    return t('shortcut.error.linuxNeedsCombination')
  }
  if (mainKeys.length === 0 && modifiers.length < 2) {
    return t('shortcut.error.needTwoModifiers')
  }

  const hasFamily = (prefix: string) => modifiers.some((code) => code.startsWith(prefix))
  const mainKey = mainKeys[0]
  if (hasFamily('Control') && hasFamily('Alt') && mainKey === 'Delete') {
    return t('shortcut.error.reservedCtrlAltDel')
  }
  // 带主键的 Win 组合由 Windows Shell 保留；Ctrl + Win 这类纯修饰键组合仍可用。
  if (!IS_LINUX && hasFamily('Meta') && mainKey) {
    return t('shortcut.error.reservedWindows')
  }
  const reservedAltMainKeys = IS_LINUX
    ? ['F4', 'Tab', 'Escape']
    : ['F4', 'Tab', 'Escape', 'Space']
  if (hasFamily('Alt')
    && (!IS_LINUX || (!hasFamily('Control') && !hasFamily('Meta')))
    && reservedAltMainKeys.includes(mainKey)) {
    return t('shortcut.error.reservedAlt')
  }
  if (hasFamily('Control') && mainKey === 'Escape') {
    return t('shortcut.error.reservedCtrlEsc')
  }

  return null
}

export function isValidPTTShortcut(
  setting: string,
  options: PTTValidationOptions = {},
): boolean {
  return getPTTShortcutValidationError(setting, options) === null
}

function pttMainCodeToAccelerator(code: string): string {
  if (code.startsWith('Key')) return code.slice(3)
  if (code.startsWith('Digit')) return code.slice(5)
  const aliases: Record<string, string> = {
    Enter: 'Return',
    ArrowUp: 'Up',
    ArrowDown: 'Down',
    ArrowLeft: 'Left',
    ArrowRight: 'Right',
  }
  return aliases[code] || code
}

/**
 * 将可由 Tauri accelerator 表示的 PTT 组合转换为 accelerator。
 * 纯修饰组合没有可靠的全局按下/松开事件，返回 undefined。
 */
export function pttShortcutToAccelerator(setting: string): string | undefined {
  if (!isValidPTTShortcut(setting)) return undefined
  const codes = parsePTTShortcut(canonicalizePTTShortcut(setting))
  const mainKeys = codes.filter((code) => !isPTTModifierCode(code))
  if (mainKeys.length !== 1) return undefined

  const hasControl = codes.some((code) => code.startsWith('Control'))
  const hasMeta = codes.some((code) => code.startsWith('Meta'))
  if (!IS_LINUX && hasControl && hasMeta) return undefined

  const parts: string[] = []
  if (IS_LINUX) {
    if (hasControl) parts.push('Control')
    if (hasMeta) parts.push('Super')
  } else if (hasControl || hasMeta) {
    // 现有 Windows 免提录制将 Ctrl/Meta 保存为 CommandOrControl。
    parts.push('CommandOrControl')
  }
  if (codes.some((code) => code.startsWith('Alt'))) parts.push('Alt')
  if (codes.some((code) => code.startsWith('Shift'))) parts.push('Shift')
  parts.push(pttMainCodeToAccelerator(mainKeys[0]))
  return parts.join('+')
}

function normalizeAccelerator(accelerator: string): string {
  const order: Record<string, number> = IS_LINUX
    ? { Control: 0, Super: 1, Alt: 2, Shift: 3 }
    : { CommandOrControl: 0, Alt: 1, Shift: 2 }
  const aliases: Record<string, string> = IS_LINUX
    ? {
        Ctrl: 'Control',
        CommandOrControl: 'Control',
        Command: 'Super',
        Meta: 'Super',
        Enter: 'Return',
        ArrowUp: 'Up',
        ArrowDown: 'Down',
        ArrowLeft: 'Left',
        ArrowRight: 'Right',
      }
    : {
        Ctrl: 'CommandOrControl',
        Control: 'CommandOrControl',
        Command: 'CommandOrControl',
        Meta: 'CommandOrControl',
        Enter: 'Return',
        ArrowUp: 'Up',
        ArrowDown: 'Down',
        ArrowLeft: 'Left',
        ArrowRight: 'Right',
      }
  return accelerator.split('+')
    .map((part) => aliases[part] || part)
    .sort((left, right) => (order[left] ?? 3) - (order[right] ?? 3) || left.localeCompare(right))
    .join('+')
}

/** 比较 PTT 物理组合与免提/预设 accelerator 是否会触发同一组合。 */
export function pttShortcutConflictsWithAccelerator(
  pttSetting: string,
  otherShortcut: string,
): boolean {
  if (!pttSetting || !otherShortcut) return false
  const pttCodes = parsePTTShortcut(canonicalizePTTShortcut(pttSetting))
  // 免提单键若同时是 PTT 组合成员，会被原生 hook 的“PTT 优先”规则完全遮蔽。
  if (isSingleKeySetting(otherShortcut) && pttCodes.includes(otherShortcut)) return true
  if (canonicalizePTTShortcut(pttSetting) === canonicalizePTTShortcut(otherShortcut)) return true
  const accelerator = pttShortcutToAccelerator(pttSetting)
  return accelerator !== undefined
    && normalizeAccelerator(accelerator) === normalizeAccelerator(otherShortcut)
}
