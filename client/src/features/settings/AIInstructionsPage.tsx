import { useEffect, useState } from 'react'
import { BUILTIN_APP_RULES } from '@/services/personalization/defaults'
import {
  getAppPromptRules,
  saveAppPromptRules,
  CUSTOM_RULE_PRIORITY,
} from '@/services/personalization/store'
import type { AppPromptRule } from '@/services/personalization/types'
import {
  refreshPreset,
  refreshRecorderSettings,
  setActivePresetCache,
  setPromptPresetsCache,
} from '@/services/recorder'
import * as bridge from '@/services/bridge'
import { DEFAULT_HANDS_FREE_SHORTCUT, DEFAULT_PTT_SHORTCUT } from '@/lib/platform'
import { useActivePreset } from '@/hooks/useActivePreset'
import { refreshActivePreset, setActivePresetKnown } from '@/stores/activePreset'
import {
  deletePromptPreset,
  getBuiltinPromptLanguage,
  getPromptPresets,
  moveCustomPromptPreset,
  getPresetShortcuts,
  getSetting,
  savePromptPreset,
  setActivePresetId,
  setBuiltinPromptLanguage,
  setPresetShortcuts,
  type BuiltinPromptLanguage,
  type PromptPreset,
} from '@/services/store'
import AIProofreadToggle from './AIProofreadToggle'
import HotwordPromptInjectToggle from './HotwordPromptInjectToggle'
import AppPromptRulesSection from './AppPromptRulesSection'
import PromptPresetSection from './PromptPresetSection'
import { useT } from '@/i18n/useT'

export default function AIInstructionsPage() {
  const t = useT()
  const [presets, setPresets] = useState<PromptPreset[]>([])
  const [promptLanguage, setPromptLanguage] = useState<BuiltinPromptLanguage>('zh-CN')
  const [promptLanguageChanging, setPromptLanguageChanging] = useState(false)
  const activePreset = useActivePreset()
  const activePresetId = activePreset.id
  const [editingPreset, setEditingPreset] = useState<PromptPreset | null>(null)
  const [appPromptRules, setAppPromptRules] = useState<AppPromptRule[]>([])
  // 编辑中的快捷键草稿：保存时才落库（取消不留痕，见 PromptPresetSection 的说明）
  const [editingShortcut, setEditingShortcut] = useState('')
  const [presetShortcuts, setPresetShortcutsState] = useState<Record<string, string>>({})

  useEffect(() => {
    void getBuiltinPromptLanguage().then(async (language) => {
      const loadedPresets = await getPromptPresets(language)
      setPromptLanguage(language)
      setPresets(loadedPresets)
    })
    getAppPromptRules().then(setAppPromptRules)
    getPresetShortcuts().then(setPresetShortcutsState)
    void refreshActivePreset()
  }, [])

  const handlePromptLanguageChange = async (language: BuiltinPromptLanguage) => {
    if (language === promptLanguage || promptLanguageChanging) return

    const previousLanguage = promptLanguage
    setPromptLanguageChanging(true)
    try {
      await setBuiltinPromptLanguage(language)
      const nextPresets = await getPromptPresets(language)
      setPromptLanguage(language)
      setPresets(nextPresets)
      setPromptPresetsCache(nextPresets)
      setEditingPreset(null)
      setEditingShortcut('')
      await refreshActivePreset()
    } catch (error) {
      console.error('[AIInstructionsPage] Failed to switch built-in prompt language:', error)
      try {
        await setBuiltinPromptLanguage(previousLanguage)
      } catch (rollbackError) {
        console.error('[AIInstructionsPage] Failed to restore built-in prompt language:', rollbackError)
      }
      setPromptLanguage(previousLanguage)
    } finally {
      setPromptLanguageChanging(false)
    }
  }

  // 预设切换快捷键不能和录音热键（免提 / 按住说话）相同，否则一次按键触发两个功能。
  // 预设之间的重复不在这里拦：handleSetPresetShortcut 会自动把旧的清掉（后设的赢）。
  const validatePresetShortcut = async (value: string): Promise<string | null> => {
    if (!value) return null
    const ptt = await getSetting('shortcutPTT', DEFAULT_PTT_SHORTCUT) as string
    const handsFree = await getSetting('shortcutHandsFree', DEFAULT_HANDS_FREE_SHORTCUT) as string
    if (value === ptt) return t('aiInstructions.conflictPtt')
    if (value === handsFree) return t('aiInstructions.conflictHandsFree')
    return null
  }

  const handleSetPresetShortcut = async (presetId: string, accel: string) => {
    const next: Record<string, string> = { ...presetShortcuts }
    if (!accel) {
      delete next[presetId]
    } else {
      // 保证组合键唯一：若其它预设已占用同一组合键，先清除，避免注册冲突
      for (const key of Object.keys(next)) {
        if (next[key] === accel) delete next[key]
      }
      next[presetId] = accel
    }
    setPresetShortcutsState(next)
    await setPresetShortcuts(next)
    bridge.notifyShortcutsChanged()
  }

  const handleSelectPreset = (id: string) => {
    // 立即更新 UI 与录音器缓存（无 IPC），持久化写入放到后台，避免快速切换时卡顿
    const target = presets.find((p) => p.id === id)
    setActivePresetKnown(id, target?.name || '')
    setActivePresetCache(id)
    void setActivePresetId(id)
  }

  const handleSavePreset = async (preset: PromptPreset) => {
    await savePromptPreset(preset)
    // 快捷键在这一刻才写入：草稿期间不动库，取消就当没发生过
    if ((presetShortcuts[preset.id] || '') !== editingShortcut) {
      await handleSetPresetShortcut(preset.id, editingShortcut)
    }
    const nextPresets = await getPromptPresets()
    setPresets(nextPresets)
    setPromptPresetsCache(nextPresets)
    setEditingPreset(null)
    setEditingShortcut('')
    // 名称可能已修改，刷新当前预设状态（标题栏/高亮）
    await refreshActivePreset()
  }

  const handleDeletePreset = async (id: string) => {
    await deletePromptPreset(id)
    const nextPresets = await getPromptPresets()
    setPresets(nextPresets)
    setPromptPresetsCache(nextPresets)
    // 清除该预设的快捷键映射，避免残留注册
    if (presetShortcuts[id]) {
      const next = { ...presetShortcuts }
      delete next[id]
      setPresetShortcutsState(next)
      await setPresetShortcuts(next)
      bridge.notifyShortcutsChanged()
    }
    if (id === activePresetId) {
      await setActivePresetId('intent')
      await refreshPreset()
      await refreshActivePreset()
    }
  }

  const handleNewPreset = () => {
    setEditingPreset({
      id: Date.now().toString(36),
      name: '',
      systemPrompt: '',
    })
    setEditingShortcut('')
  }

  /** 开始编辑：把该模式已有的快捷键读进草稿 */
  const handleStartEditing = (preset: PromptPreset) => {
    setEditingPreset(preset)
    setEditingShortcut(presetShortcuts[preset.id] || '')
  }

  const handleCancelEditing = () => {
    setEditingPreset(null)
    setEditingShortcut('')
  }

  const handleMovePreset = async (from: number, to: number) => {
    await moveCustomPromptPreset(from, to)
    const nextPresets = await getPromptPresets()
    setPresets(nextPresets)
    setPromptPresetsCache(nextPresets)
  }

  const handleSaveAppRule = async (rule: AppPromptRule) => {
    const nextRules = appPromptRules
      .map((item) => (item.id === rule.id ? rule : item))
      .sort((left, right) => right.priority - left.priority)
    setAppPromptRules(nextRules)
    await saveAppPromptRules(nextRules)
    await refreshRecorderSettings()
  }

  const handleCreateAppRule = async (draft: {
    name: string
    processNames: string[]
    presetId?: string
    promptAppend: string
  }) => {
    const id = `custom_${Date.now().toString(36)}_${Math.random().toString(36).slice(2, 6)}`
    const rule: AppPromptRule = {
      id,
      appId: id,
      name: draft.name,
      builtin: false,
      enabled: true,
      priority: CUSTOM_RULE_PRIORITY,
      presetId: draft.presetId,
      promptAppend: draft.promptAppend,
      matcher: { processNames: draft.processNames, windowTitleIncludes: [], windowClasses: [], automationIds: [] },
    }
    const nextRules = [...appPromptRules, rule].sort((left, right) => right.priority - left.priority)
    setAppPromptRules(nextRules)
    await saveAppPromptRules(nextRules)
    await refreshRecorderSettings()
  }

  const handleDeleteAppRule = async (ruleId: string) => {
    const nextRules = appPromptRules.filter((rule) => rule.id !== ruleId)
    setAppPromptRules(nextRules)
    await saveAppPromptRules(nextRules)
    await refreshRecorderSettings()
  }

  const handleResetAppRule = async (ruleId: string) => {
    const fallback = BUILTIN_APP_RULES.find((rule) => rule.id === ruleId)
    if (!fallback) return
    const nextRules = appPromptRules
      .map((rule) => (rule.id === ruleId ? { ...fallback, matcher: { ...fallback.matcher } } : rule))
      .sort((left, right) => right.priority - left.priority)
    setAppPromptRules(nextRules)
    await saveAppPromptRules(nextRules)
    await refreshRecorderSettings()
  }

  return (
    <div className="mx-auto max-w-4xl">
      <h1 className="mb-2 text-2xl font-bold">{t('nav.aiInstructions')}</h1>
      <p className="mb-6 text-sm text-muted-foreground">
        {t('aiInstructions.subtitle')}
      </p>

      <div className="space-y-6">
        <AIProofreadToggle />
        <HotwordPromptInjectToggle />

        <PromptPresetSection
          presets={presets}
          activePresetId={activePresetId}
          editingPreset={editingPreset}
          presetShortcuts={presetShortcuts}
          editingShortcut={editingShortcut}
          promptLanguage={promptLanguage}
          promptLanguageChanging={promptLanguageChanging}
          validateShortcut={validatePresetShortcut}
          onSelectPreset={handleSelectPreset}
          onPromptLanguageChange={handlePromptLanguageChange}
          onStartNewPreset={handleNewPreset}
          onStartEditing={handleStartEditing}
          onEditingPresetChange={setEditingPreset}
          onEditingShortcutChange={setEditingShortcut}
          onCancelEditing={handleCancelEditing}
          onSavePreset={handleSavePreset}
          onDeletePreset={handleDeletePreset}
          onMovePreset={handleMovePreset}
        />

        <AppPromptRulesSection
          presets={presets}
          rules={appPromptRules}
          onSaveRule={handleSaveAppRule}
          onResetRule={handleResetAppRule}
          onCreateRule={handleCreateAppRule}
          onDeleteRule={handleDeleteAppRule}
        />
      </div>
    </div>
  )
}
