import { useEffect, useRef, useState } from 'react'
import { Mic, Sparkles, Globe, ArrowRight, ArrowLeft, CheckCircle2, AlertCircle, Keyboard } from 'lucide-react'
import { Card, CardContent } from '@/components/ui/card'
import { getSetting, setSetting } from '@/services/store'
import { getWorkMode } from '@/services/transcription'
import { healthCheck } from '@/services/api'
import { setPttSuppressed } from '@/services/recorder'
import * as bridge from '@/services/bridge'
import {
  displayShortcut,
  getSingleKeyDisplay,
  isSingleKeySetting,
} from '@/lib/shortcutKeys'
import { DEFAULT_HANDS_FREE_SHORTCUT, IS_LINUX } from '@/lib/platform'
import { eventToAccelerator } from '@/features/settings/utils'
import { refreshPTTSetting } from '@/services/webviewKeyboardFallback'
import appIcon from '@/assets/icon-128.png'
import { t, type TranslationKey } from '@/i18n'
import { useT } from '@/i18n/useT'

/** 简化键盘布局，高亮当前快捷键 */
function KeyboardHint({ activeKey, pressed }: { activeKey: string; pressed?: boolean }) {
  const rows: { code: string; label: string; w?: number }[][] = [
    [
      { code: 'ShiftLeft', label: 'Shift', w: 52 },
      { code: 'KeyZ', label: 'Z' }, { code: 'KeyX', label: 'X' },
      { code: 'KeyC', label: 'C' }, { code: 'KeyV', label: 'V' },
      { code: 'KeyB', label: 'B' }, { code: 'KeyN', label: 'N' },
      { code: 'KeyM', label: 'M' }, { code: 'Comma', label: '<' },
      { code: 'Period', label: '>' }, { code: 'Slash', label: '?' },
      { code: 'ShiftRight', label: 'Shift', w: 52 },
    ],
    [
      { code: 'ControlLeft', label: 'Ctrl', w: 42 },
      { code: 'MetaLeft', label: 'Win', w: 34 },
      { code: 'AltLeft', label: 'Alt', w: 34 },
      { code: 'Space', label: t('keyName.Space'), w: 148 },
      { code: 'AltRight', label: 'Alt', w: 34 },
      { code: 'ControlRight', label: 'Ctrl', w: 42 },
    ],
  ]

  return (
    <div className="flex flex-col items-center gap-1">
      {rows.map((row, ri) => (
        <div key={ri} className="flex items-center justify-center gap-1">
          {row.map((key) => {
            const isActive = key.code === activeKey
            const isPressed = isActive && pressed
            return (
              <div
                key={key.code}
                className={`flex items-center justify-center rounded-md transition-all ${isPressed
                  ? 'bg-foreground text-background font-bold shadow-lg ring-2 ring-foreground/30 scale-110'
                  : isActive
                    ? 'bg-primary text-primary-foreground font-semibold shadow-sm ring-2 ring-primary/30'
                    : 'border border-border/60 bg-muted/30 text-muted-foreground'
                  }`}
                style={{
                  width: isActive ? (key.w || 30) + 4 : key.w || 30,
                  height: isActive ? 32 : 28,
                  fontSize: isActive ? 12 : 10,
                }}
              >
                {key.label}
              </div>
            )
          })}
        </div>
      ))}
    </div>
  )
}

const MODE_LABEL_KEYS: Record<string, TranslationKey> = {
  server: 'mode.server',
  cloud_api: 'mode.cloudApi',
  local: 'mode.local',
}

/**
 * 把带 `{key}` 占位的句子渲染成「文字 + 加粗键名 + 文字」。
 * 按占位符切分，键名在句中的位置就由译文决定（中英语序不同）。
 */
function HotkeyPrompt({ template, keyLabel }: { template: string; keyLabel: string }) {
  const [before, after = ''] = template.split('{key}')
  return (
    <>
      {before}
      <span className="font-medium text-foreground">{keyLabel}</span>
      {after}
    </>
  )
}

/** 同 HotkeyPrompt，只是键名用键帽样式。 */
function ReadyHint({ template, keyLabel }: { template: string; keyLabel: string }) {
  const [before, after = ''] = template.split('{key}')
  return (
    <>
      {before}
      <span className="rounded border border-border bg-muted/50 px-1.5 py-0.5 font-medium text-foreground">{keyLabel}</span>
      {after}
    </>
  )
}

interface WelcomeGuideProps {
  onComplete: () => void
}

export default function WelcomeGuide({ onComplete }: WelcomeGuideProps) {
  const t = useT()
  const [step, setStep] = useState(0)
  const [hfKey, setHfKey] = useState(DEFAULT_HANDS_FREE_SHORTCUT)
  const hfLabel = displayShortcut(hfKey).join(' + ')
  const [workMode, setWorkMode] = useState('')
  const [serverOk, setServerOk] = useState<boolean | null>(null)
  const [testText, setTestText] = useState('')
  const [listeningKey, setListeningKey] = useState(false)
  // 热键确认步骤的状态
  const [keyConfirmed, setKeyConfirmed] = useState(false)
  const [keyPressed, setKeyPressed] = useState(false)
  const keyPressedRef = useRef(false)
  const hfKeyRef = useRef(DEFAULT_HANDS_FREE_SHORTCUT)
  const settingsDirtyRef = useRef(false)

  useEffect(() => {
    getSetting('shortcutHandsFree', DEFAULT_HANDS_FREE_SHORTCUT).then((k) => {
      const key = k as string
      setHfKey(key)
      hfKeyRef.current = key
    })
    const mode = getWorkMode()
    setWorkMode(mode)
    if (mode === 'server') {
      healthCheck().then(() => setServerOk(true)).catch(() => setServerOk(false))
    }
  }, [])

  // 热键确认步骤：监听按键按下和松开，同时抑制录音系统响应热键
  // 只依赖 step，避免 hfKey 变化导致 effect 反复运行和钩子反复重启
  useEffect(() => {
    if (step !== 2) return
    setPttSuppressed(true)
    const pressedKeyCodeRef = { current: '' } // 当前按住的键

    const confirmKey = (code: string) => {
      // 保存到 ref，避免 state 更新触发 effect 重启
      hfKeyRef.current = code
      settingsDirtyRef.current = true
      // 更新 UI 状态（state 更新不会触发 effect 重建，因为 state 不在依赖里）
      setHfKey(code)
      setKeyPressed(true)
      keyPressedRef.current = true
      pressedKeyCodeRef.current = code
      setKeyConfirmed(true)
    }

    const releaseKey = () => {
      if (keyPressedRef.current) {
        keyPressedRef.current = false
        setKeyPressed(false)
      }
      pressedKeyCodeRef.current = ''
    }

    // 路径 1：webview 能收到的按键（右 Ctrl 等）
    const onDown = (e: KeyboardEvent) => {
      e.preventDefault()
      const code = e.code
      if (!IS_LINUX && isSingleKeySetting(code) && pressedKeyCodeRef.current !== code) {
        confirmKey(code)
      }
    }
    const onUp = (e: KeyboardEvent) => {
      e.preventDefault()
      if (pressedKeyCodeRef.current === e.code) {
        releaseKey()
      }
    }

    // 路径 2：被 Rust 钩子拦截的按键（右 Alt）—— keyup 时触发
    const unlistenHf = bridge.listen('toggle-hands-free', () => {
      // Rust 端只在 keyup 时 emit，模拟"按下-松开"的视觉反馈
      confirmKey(hfKeyRef.current || DEFAULT_HANDS_FREE_SHORTCUT)
      setTimeout(() => releaseKey(), 150)
    })

    // 路径 3：PTT 键 keydown/keyup
    const unlistenPttDown = bridge.listen('ptt-down', (event: unknown) => {
      const payload = event as { payload?: { pttSetting?: string } }
      const setting = payload?.payload?.pttSetting
      if (setting && isSingleKeySetting(setting) && pressedKeyCodeRef.current !== setting) {
        confirmKey(setting)
      }
    })
    const unlistenPttUp = bridge.listen('ptt-up', () => {
      releaseKey()
    })

    window.addEventListener('keydown', onDown)
    window.addEventListener('keyup', onUp)
    return () => {
      window.removeEventListener('keydown', onDown)
      window.removeEventListener('keyup', onUp)
      setPttSuppressed(false)
      unlistenHf.then((fn) => fn())
      unlistenPttDown.then((fn) => fn())
      unlistenPttUp.then((fn) => fn())

      // 离开 Step 2 时统一保存设置并重配置键盘钩子
      if (settingsDirtyRef.current) {
        settingsDirtyRef.current = false
        void (async () => {
          await setSetting('shortcutHandsFree', hfKeyRef.current)
          bridge.notifyShortcutsChanged()
          // 同步刷新 webview 回退缓存，否则向导内（SayIt 窗口聚焦）测试时新键不生效
          await refreshPTTSetting()
        })()
      }
    }
  }, [step])

  // 试一试步骤：监听按键修改快捷键
  useEffect(() => {
    if (!listeningKey) return
    const handler = (e: KeyboardEvent) => {
      e.preventDefault()
      const shortcut = IS_LINUX ? eventToAccelerator(e) : (isSingleKeySetting(e.code) ? e.code : null)
      if (shortcut) {
        setHfKey(shortcut)
        void (async () => {
          await setSetting('shortcutHandsFree', shortcut)
          bridge.notifyShortcutsChanged()
          // 同步刷新 webview 回退缓存，否则向导内测试时新键不生效
          await refreshPTTSetting()
        })()
        setListeningKey(false)
      }
    }
    window.addEventListener('keydown', handler)
    return () => window.removeEventListener('keydown', handler)
  }, [listeningKey])

  const canTest = workMode === 'server' && serverOk === true
  const totalSteps = 5
  const isLast = step === totalSteps - 1

  const renderStep = () => {
    switch (step) {
      // Step 0: 欢迎
      case 0:
        return (
          <div className="flex flex-col items-center text-center">
            <img src={appIcon} alt="SayIt" className="mb-6 h-20 w-20 rounded-2xl" />
            <h1 className="text-3xl font-bold tracking-tight" style={{ fontFamily: "'Nunito', sans-serif", fontWeight: 800 }}>{t('welcome.title')}</h1>
            <p className="mt-3 text-base text-muted-foreground">{t('welcome.tagline')}</p>
            <p className="mt-1.5 text-sm text-muted-foreground/70">{t('welcome.subtitle')}</p>
          </div>
        )

      // Step 1: 核心功能
      case 1:
        return (
          <div>
            <h2 className="mb-5 text-center text-xl font-bold">{t('welcome.featuresTitle')}</h2>
            <div className="space-y-3">
              {[
                { icon: Mic, title: t('welcome.feature.handsFree.title'), desc: t('welcome.feature.handsFree.desc'), color: 'text-blue-500 bg-blue-500/10' },
                { icon: Sparkles, title: t('welcome.feature.ai.title'), desc: t('welcome.feature.ai.desc'), color: 'text-amber-500 bg-amber-500/10' },
                { icon: Globe, title: t('welcome.feature.deploy.title'), desc: t('welcome.feature.deploy.desc'), color: 'text-emerald-500 bg-emerald-500/10' },
              ].map(({ icon: Icon, title, desc, color }) => (
                <Card key={title}>
                  <CardContent className="flex items-center gap-4 p-4">
                    <div className={`flex h-10 w-10 shrink-0 items-center justify-center rounded-xl ${color}`}>
                      <Icon className="h-5 w-5" />
                    </div>
                    <div>
                      <p className="text-sm font-medium">{title}</p>
                      <p className="mt-0.5 text-xs text-muted-foreground">{desc}</p>
                    </div>
                  </CardContent>
                </Card>
              ))}
            </div>
          </div>
        )

      // Step 2: 热键确认
      case 2:
        return (
          <div className="flex flex-col items-center text-center">
            <div className="mb-5 flex h-14 w-14 items-center justify-center rounded-2xl bg-primary/10">
              <Keyboard className="h-7 w-7 text-primary" />
            </div>
            <h2 className="text-xl font-bold">{t('welcome.hotkeyTitle')}</h2>
            <p className="mt-2 text-sm text-muted-foreground">
              {keyConfirmed
                ? t('welcome.hotkeyConfirmed')
                : <HotkeyPrompt template={t('welcome.hotkeyPrompt')} keyLabel={IS_LINUX ? hfLabel : getSingleKeyDisplay('AltRight')} />}
            </p>

            {/* 大号按键展示 */}
            <div className="my-6">
              <div
                className={`inline-flex items-center justify-center rounded-xl border-2 px-8 py-4 text-lg font-bold transition-all ${keyPressed
                  ? 'border-foreground bg-foreground text-background scale-105 shadow-xl'
                  : keyConfirmed
                    ? 'border-emerald-500 bg-emerald-500/10 text-emerald-600'
                    : 'border-primary/40 bg-primary/5 text-primary animate-pulse'
                  }`}
              >
                {keyPressed ? `${hfLabel} ⬇` : keyConfirmed ? `✓ ${hfLabel}` : hfLabel}
              </div>
            </div>

            {/* 键盘位置示意 */}
            {!IS_LINUX && (
              <div className="w-full rounded-xl border bg-card p-3">
                <p className="mb-2 text-[11px] text-muted-foreground">{t('welcome.keyPosition')}</p>
                <KeyboardHint activeKey={hfKey} pressed={keyPressed} />
              </div>
            )}

            {!keyConfirmed && (
              <p className="mt-4 text-xs text-muted-foreground/60">
                {t('welcome.supportedKeys')}
              </p>
            )}
            {keyConfirmed && (
              <p className="mt-4 text-xs text-muted-foreground">
                {t('welcome.changeHint')}
              </p>
            )}
          </div>
        )

      // Step 3: 试一试
      case 3:
        return (
          <div>
            <h2 className="mb-5 text-center text-xl font-bold">{t('welcome.tryTitle')}</h2>

            <Card className="mb-4">
              <CardContent className="flex items-center justify-between p-4">
                <div className="flex items-center gap-3">
                  <div className="flex h-10 w-10 items-center justify-center rounded-xl bg-primary/10">
                    <Keyboard className="h-5 w-5 text-primary" />
                  </div>
                  <div>
                    <p className="text-sm font-medium">{t('welcome.handsFreeShortcut')}</p>
                    <p className="mt-0.5 text-xs text-muted-foreground">{t('welcome.clickToChange')}</p>
                  </div>
                </div>
                <button
                  onClick={() => setListeningKey(true)}
                  className={`rounded-lg border px-3 py-1.5 text-sm font-semibold transition-all ${listeningKey
                    ? 'border-primary bg-primary/10 text-primary animate-pulse'
                    : 'border-border bg-muted/50 text-foreground hover:border-primary/50'
                    }`}
                >
                  {listeningKey ? t('welcome.pressNewKey') : hfLabel}
                </button>
              </CardContent>
            </Card>

            <Card className="mb-4">
              <CardContent className="flex items-center justify-between p-4">
                <div>
                  <p className="text-sm font-medium">{t('welcome.currentMode')}</p>
                  <p className="mt-0.5 text-xs text-muted-foreground">{MODE_LABEL_KEYS[workMode] ? t(MODE_LABEL_KEYS[workMode]) : workMode}</p>
                </div>
                {workMode === 'server' && (
                  <div className="flex items-center gap-1.5">
                    {serverOk === null ? (
                      <span className="text-xs text-muted-foreground">{t('common.checking')}</span>
                    ) : serverOk ? (
                      <><CheckCircle2 className="h-4 w-4 text-emerald-500" /><span className="text-xs text-emerald-600">{t('welcome.connected')}</span></>
                    ) : (
                      <><AlertCircle className="h-4 w-4 text-destructive" /><span className="text-xs text-destructive">{t('welcome.notConnected')}</span></>
                    )}
                  </div>
                )}
                {workMode !== 'server' && (
                  <span className="text-xs text-muted-foreground">{t('welcome.needsSetup')}</span>
                )}
              </CardContent>
            </Card>

            <Card>
              <CardContent className="p-4">
                <p className="mb-2 text-sm font-medium">{canTest ? t('welcome.voiceTestReady') : t('welcome.voiceTest')}</p>
                {canTest && (
                  <p className="mb-2 text-xs text-muted-foreground">{t('welcome.testHint', { key: hfLabel })}</p>
                )}
                <textarea
                  value={testText}
                  onChange={(e) => setTestText(e.target.value)}
                  placeholder={canTest
                    ? t('welcome.testPlaceholder', { key: hfLabel })
                    : t('welcome.testUnavailable')}
                  className="w-full resize-none rounded-lg border border-input-border bg-input-bg p-3 text-sm leading-relaxed placeholder:text-muted-foreground/40 focus:border-input-focus-border focus:outline-none"
                  rows={4}
                  readOnly={!canTest}
                />
                {!canTest && (
                  <p className="mt-2 text-xs text-muted-foreground">
                    {t('welcome.testUnavailableHint', { key: hfLabel })}
                  </p>
                )}
              </CardContent>
            </Card>
          </div>
        )

      // Step 4: 完成
      case 4:
        return (
          <div className="flex flex-col items-center text-center">
            <div className="mb-6 flex h-16 w-16 items-center justify-center rounded-full bg-emerald-500/10">
              <CheckCircle2 className="h-8 w-8 text-emerald-500" />
            </div>
            <h2 className="text-xl font-bold">{t('welcome.readyTitle')}</h2>
            <p className="mt-3 text-sm text-muted-foreground">
              <ReadyHint template={t('welcome.readyHint')} keyLabel={hfLabel} />
            </p>
            <p className="mt-1.5 text-xs text-muted-foreground/70">{t('welcome.readySubHint')}</p>
          </div>
        )

      default:
        return null
    }
  }

  return (
    <div className="fixed inset-0 z-[100] flex items-center justify-center bg-background/95 backdrop-blur-sm">
      <div className="custom-scrollbar w-full max-w-lg max-h-[90vh] overflow-y-auto px-6 py-8">
        {renderStep()}

        {/* 底部导航 */}
        <div className="mt-8 flex items-center justify-between">
          <div className="flex gap-1.5">
            {Array.from({ length: totalSteps }, (_, i) => (
              <div
                key={i}
                className={`h-1.5 rounded-full transition-all ${i === step ? 'w-6 bg-primary' : 'w-1.5 bg-muted-foreground/20'
                  }`}
              />
            ))}
          </div>
          <div className="flex gap-2">
            {step > 0 && (
              <button
                onClick={() => setStep(step - 1)}
                className="flex items-center gap-1 rounded-lg px-4 py-2 text-sm text-muted-foreground transition-colors hover:text-foreground"
              >
                <ArrowLeft className="h-3.5 w-3.5" />
                {t('common.back')}
              </button>
            )}
            {!isLast && step === 0 && (
              <button
                onClick={onComplete}
                className="rounded-lg px-4 py-2 text-sm text-muted-foreground transition-colors hover:text-foreground"
              >
                {t('common.skip')}
              </button>
            )}
            <button
              onClick={() => isLast ? onComplete() : setStep(step + 1)}
              className="flex items-center gap-1.5 rounded-lg bg-primary px-5 py-2 text-sm font-medium text-primary-foreground transition-colors hover:bg-primary/90"
            >
              {isLast ? t('welcome.start') : t('common.next')}
              <ArrowRight className="h-3.5 w-3.5" />
            </button>
          </div>
        </div>
      </div>
    </div>
  )
}
