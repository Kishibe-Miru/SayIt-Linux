import * as bridge from '../services/bridge'
import { useEffect, useMemo, useRef, useState } from 'react'
import { Copy, Check, X } from 'lucide-react'
import { isLocale, setLocale } from '@/i18n'
import { useT } from '@/i18n/useT'
import { addRuntimeEvent } from '../services/debugLog'
import { formatRecordingTimer } from '../services/recorder/types'

type OverlayState = 'waiting' | 'listening' | 'thinking' | 'fallback' | 'error' | 'toast'
type OverlayWaveTheme = 'black-white' | 'black-blue' | 'black-rainbow'

interface OverlayPayload {
  state?: OverlayState
  bars?: number[]
  elapsedSec?: number
  theme?: OverlayWaveTheme
  showDuration?: boolean
  barCount?: number
  fallbackText?: string
  fallbackReason?: string
  errorMessage?: string
  warning?: string
  /** warning 的严重级别：warn=琥珀（声音小/未检测到），error=红色高警（麦克风已被静音） */
  warningTone?: 'warn' | 'error'
  toastText?: string
  /** toast 的语气：info=中性（如切换预设），warn=琥珀色+图标（如未检测到声音） */
  toastTone?: 'info' | 'warn'
  streaming?: boolean
  streamingText?: string
  /** 界面语言，由主窗随每次更新下发（见 OverlayCommonPayload.locale）。 */
  locale?: string
  _overlayShowId?: number
  _overlayGeneration?: number
  _overlayProbe?: boolean
}

const DEFAULT_BAR_COUNT = 24
const IDLE_BARS = Array(DEFAULT_BAR_COUNT).fill(3)

function normalizeTheme(theme: unknown): OverlayWaveTheme {
  if (theme === 'black-white' || theme === 'black-blue' || theme === 'black-rainbow') {
    return theme
  }
  return 'black-white'
}

function getListeningBarColor(index: number, total: number, theme: OverlayWaveTheme): string {
  const safeTotal = Math.max(1, total - 1)
  const t = index / safeTotal

  if (theme === 'black-white') {
    return '#f1f5f9'
  }

  if (theme === 'black-rainbow') {
    const hue = 140 - Math.round(t * 110)
    const lightness = 64 - Math.round(Math.abs(t - 0.5) * 12)
    return `hsl(${hue} 95% ${lightness}%)`
  }

  const hue = 190 + Math.round(t * 30)
  const lightness = 62 - Math.round(Math.abs(t - 0.5) * 14)
  return `hsl(${hue} 90% ${lightness}%)`
}

function getTimerColor(theme: OverlayWaveTheme): string {
  if (theme === 'black-white') return '#e5e7eb'
  if (theme === 'black-rainbow') return '#fef08a'
  return '#bae6fd'
}

function getThinkingColor(theme: OverlayWaveTheme): string {
  if (theme === 'black-white') return '#e2e8f0'
  if (theme === 'black-rainbow') return '#facc15'
  return '#38bdf8'
}

export default function Overlay() {
  const t = useT()
  const [state, setState] = useState<OverlayState>('waiting')
  const [bars, setBars] = useState<number[]>(IDLE_BARS)
  const [elapsedSec, setElapsedSec] = useState(0)
  const [theme, setTheme] = useState<OverlayWaveTheme>('black-white')
  const [showDuration, setShowDuration] = useState(true)
  const [barCount, setBarCount] = useState(DEFAULT_BAR_COUNT)
  const [fallbackText, setFallbackText] = useState('')
  const [errorMessage, setErrorMessage] = useState('')
  const [toastText, setToastText] = useState('')
  const [toastTone, setToastTone] = useState<'info' | 'warn'>('info')
  const [streamingText, setStreamingText] = useState('')
  const [streamingOn, setStreamingOn] = useState(false)
  const [copied, setCopied] = useState(false)
  const [thinkingDuration, setThinkingDuration] = useState(0)
  const [warning, setWarning] = useState('')
  const [warningTone, setWarningTone] = useState<'warn' | 'error'>('warn')
  const rootRef = useRef<HTMLDivElement | null>(null)
  const hideTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  const elapsedSecRef = useRef(0)

  // 根据录音时长计算预估处理时间（秒）
  const calculateThinkingDuration = (recordingSec: number): number => {
    if (recordingSec <= 5) return 2
    if (recordingSec <= 15) return 3
    if (recordingSec <= 30) return 4
    if (recordingSec <= 60) return 5
    if (recordingSec <= 120) return 7
    if (recordingSec <= 180) return 9
    if (recordingSec <= 240) return 11
    return 13 // 240秒以上（4-5分钟）
  }

  useEffect(() => {
    let disposed = false
    let removeOverlayListener: (() => void) | null = null

    const handleOverlayState = (data: unknown) => {
      const payload = data as OverlayPayload
      // 语言先落地再渲染本帧：悬浮窗没有自己的初始化时机，语言只能随 payload 来。
      // setLocale 对同值是空操作，所以每帧都调也不会造成额外重渲染。
      if (isLocale(payload.locale)) setLocale(payload.locale)
      const nextElapsedSec = typeof payload.elapsedSec === 'number'
        ? payload.elapsedSec
        : elapsedSecRef.current

      if (payload.state) {
        setState(payload.state)
        if (payload.state !== 'listening') {
          setBars((prev) => Array(prev.length).fill(3))
          // 离开录音状态即清空实时文字气泡
          setStreamingText('')
          setStreamingOn(false)
        }
        setCopied(false)
        if (payload.state !== 'fallback' && hideTimerRef.current) {
          clearTimeout(hideTimerRef.current)
          hideTimerRef.current = null
        }
        if (payload.state === 'thinking') {
          setThinkingDuration(calculateThinkingDuration(nextElapsedSec))
        }
      }

      if (Array.isArray(payload.bars) && payload.bars.length > 0) setBars(payload.bars)
      if (typeof payload.elapsedSec === 'number') {
        elapsedSecRef.current = payload.elapsedSec
        setElapsedSec(payload.elapsedSec)
      }
      if (typeof payload.showDuration === 'boolean') setShowDuration(payload.showDuration)
      if (payload.theme) setTheme(normalizeTheme(payload.theme))
      if (typeof payload.barCount === 'number' && payload.barCount > 0) setBarCount(payload.barCount)
      if (typeof payload.fallbackText === 'string') setFallbackText(payload.fallbackText)
      if (typeof payload.errorMessage === 'string') setErrorMessage(payload.errorMessage)
      if (typeof payload.toastText === 'string') setToastText(payload.toastText)
      if (payload.toastTone === 'info' || payload.toastTone === 'warn') setToastTone(payload.toastTone)
      if (typeof payload.warning === 'string') setWarning(payload.warning)
      if (payload.warningTone === 'warn' || payload.warningTone === 'error') setWarningTone(payload.warningTone)
      if (typeof payload.streamingText === 'string') setStreamingText(payload.streamingText)
      if (typeof payload.streaming === 'boolean') setStreamingOn(payload.streaming)
      if (payload.state === 'waiting') {
        setElapsedSec(0)
        setWarning('')
        setWarningTone('warn')
        setStreamingText('')
        setStreamingOn(false)
        setBars((prev) => Array(prev.length).fill(3))
      }

      if (!payload._overlayProbe) return
      const showId = payload._overlayShowId
      const generation = payload._overlayGeneration
      if (typeof showId !== 'number' || typeof generation !== 'number') return

      requestAnimationFrame(() => {
        requestAnimationFrame(() => {
          if (disposed) return
          const root = rootRef.current
          const content = root?.querySelector<HTMLElement>('[data-overlay-content]') ?? null
          const rootRect = root?.getBoundingClientRect()
          const contentRect = content?.getBoundingClientRect()
          const style = content ? window.getComputedStyle(content) : null
          const healthy = Boolean(
            rootRect && contentRect
            && rootRect.width > 0 && rootRect.height > 0
            && contentRect.width > 0 && contentRect.height > 0
            && style?.display !== 'none'
            && style?.visibility !== 'hidden'
            && Number(style?.opacity ?? '1') > 0
          )
          void bridge.overlayRenderAck({
            showId,
            generation,
            healthy,
            overlayState: payload.state ?? 'unknown',
            documentVisibility: document.visibilityState,
            rootWidth: rootRect?.width ?? 0,
            rootHeight: rootRect?.height ?? 0,
            contentWidth: contentRect?.width ?? 0,
            contentHeight: contentRect?.height ?? 0,
            display: style?.display ?? 'missing',
            visibility: style?.visibility ?? 'missing',
            opacity: style?.opacity ?? 'missing',
          }).catch(() => { })
        })
      })
    }

    void bridge.listen<unknown>('overlay-state', (event) => handleOverlayState(event.payload))
      .then((unlisten) => {
        if (disposed) {
          unlisten()
          return
        }
        removeOverlayListener = unlisten
        void bridge.overlayReady().catch(() => { })
      })

    return () => {
      disposed = true
      removeOverlayListener?.()
      if (hideTimerRef.current) {
        clearTimeout(hideTimerRef.current)
        hideTimerRef.current = null
      }
    }
  }, [])

  const showStreamingBubble = state === 'listening' && (streamingOn || streamingText.trim().length > 0)
  const hasStreamingText = streamingText.trim().length > 0

  const { text: timerText, countdown: inCountdown, remainingSec } = useMemo(
    () => formatRecordingTimer(elapsedSec),
    [elapsedSec],
  )
  // 越接近上限越醒目，但不用刺眼的纯红：琥珀 → 暖橙，最后 10 秒再叠一层脉动。
  // 悬浮窗是黑底小尺寸，纯红（#f87171）在这上面又艳又跳，和整体配色不搭。
  const urgent = inCountdown && remainingSec <= 10
  // 胶囊宽度按波形条数固定，文字挤进来时按需让出条数：
  //   · 同时有警告文字 + 倒计时 → 只画一半（最少 4 根，太少就不像波形了）
  //   · 只有其中一种 → 画三分之二
  // 这样波形始终是完整的整根条，不会被裁成半截。
  const barBudget = warning && inCountdown
    ? Math.max(4, Math.floor(bars.length / 2))
    : (warning || inCountdown ? Math.max(6, Math.floor((bars.length * 2) / 3)) : bars.length)
  const visibleBars = barBudget >= bars.length ? bars : bars.slice(0, barBudget)

  const timerColor = inCountdown
    ? (urgent ? '#fb923c' : '#fbbf24')
    : getTimerColor(theme)
  const thinkingColor = getThinkingColor(theme)

  const handleCopyFallback = async () => {
    if (!fallbackText) return

    try {
      await bridge.copyText(fallbackText)
      setCopied(true)
      addRuntimeEvent('info', 'overlay', 'Fallback card copied', { textLen: fallbackText.length })

      if (hideTimerRef.current) {
        clearTimeout(hideTimerRef.current)
      }
      hideTimerRef.current = setTimeout(() => {
        void bridge.setEscapeActionMode('off')
        void bridge.hideOverlay()
        hideTimerRef.current = null
      }, 500)
    } catch (error) {
      addRuntimeEvent('error', 'overlay', 'Fallback card copy failed', { error: String(error) })
    }
  }

  const handleDismissFallback = () => {
    addRuntimeEvent('info', 'overlay', 'Fallback card dismissed by user')
    void bridge.setEscapeActionMode('off')
    void bridge.hideOverlay()
  }

  return (
    <div
      ref={rootRef}
      className="pointer-events-none flex h-full items-end justify-center pb-4"
    >
      {state === 'fallback' ? (
        <div
          data-overlay-content
          className="pointer-events-auto flex w-full max-w-[520px] flex-col rounded-xl border px-4 py-4"
          style={{
            background: 'var(--overlay-bg)',
            color: 'var(--overlay-text)',
            borderColor: 'var(--overlay-border)',
          }}
        >
          <div className="flex items-start justify-between gap-3">
            <div className="space-y-1">
              <span className="block text-xs font-medium tracking-[0.16em]" style={{ color: 'var(--overlay-text-muted)' }}>{t('overlay.recognizedText')}</span>
              <span className="block text-xs" style={{ color: 'var(--overlay-text-dim)' }}>
                {t('overlay.fallbackHint')}
              </span>
            </div>
            <div className="flex shrink-0 items-center gap-2">
              <button
                type="button"
                onClick={handleCopyFallback}
                title={copied ? t('overlay.copied') : t('overlay.copyText')}
                className={`inline-flex h-8 w-8 items-center justify-center rounded-lg border transition-colors ${copied
                  ? 'border-emerald-400/40 bg-emerald-500/15 text-emerald-200'
                  : 'border-white/10 bg-white/10 text-white/90 hover:bg-white/20'
                  }`}
              >
                {copied ? <Check className="h-4 w-4" /> : <Copy className="h-4 w-4" />}
              </button>
              <button
                type="button"
                onClick={handleDismissFallback}
                title={t('window.close')}
                aria-label={t('overlay.dismissAria')}
                className="inline-flex h-8 w-8 items-center justify-center rounded-lg border border-white/10 bg-white/5 text-white/60 transition-colors hover:bg-white/15 hover:text-white"
              >
                <X className="h-4 w-4" />
              </button>
            </div>
          </div>
          <div className="mt-4 flex-1 overflow-hidden rounded-lg px-3 py-3" style={{ background: 'var(--overlay-surface)' }}>
            <p className="max-h-[108px] overflow-auto pr-1 text-sm leading-6 select-text">
              {fallbackText || t('overlay.noText')}
            </p>
          </div>
        </div>
      ) : (
        <div data-overlay-content className="flex flex-col items-center gap-2">
          {showStreamingBubble && (
            <div
              className="pointer-events-none relative flex max-w-[440px] flex-col rounded-2xl border px-4 py-2.5"
              style={{
                background: 'var(--overlay-bg)',
                color: 'var(--overlay-text)',
                borderColor: 'var(--overlay-border)',
              }}
            >
              <span
                className="mb-1 text-[10px] font-medium tracking-[0.18em]"
                style={{ color: 'var(--overlay-text-muted)' }}
              >
                {t('overlay.liveCaption')}
              </span>
              {/* 内容驱动尺寸：小默认，随文字增行慢慢变大，超过 3 行才滚动，只显示最新内容 */}
              <div
                className="flex max-h-[72px] flex-col justify-end overflow-hidden text-left text-sm leading-6"
                style={{ color: hasStreamingText ? 'var(--overlay-text)' : 'var(--overlay-text-dim)' }}
              >
                <div>
                  {hasStreamingText ? streamingText : t('overlay.listening')}
                  {hasStreamingText && (
                    <span
                      className="ml-0.5 inline-block h-[1.05em] w-[2px] rounded-full align-middle"
                      style={{
                        backgroundColor: 'var(--overlay-text)',
                        animation: 'caret-blink 1.1s ease-in-out infinite',
                      }}
                    />
                  )}
                </div>
              </div>
              {/* 底部朝下尖角：提示文字来自下方的录音胶囊 */}
              <span
                className="absolute left-1/2 h-0 w-0 -translate-x-1/2"
                style={{
                  bottom: '-7px',
                  borderLeft: '7px solid transparent',
                  borderRight: '7px solid transparent',
                  borderTop: '7px solid var(--overlay-bg)',
                }}
              />
            </div>
          )}
          {/* 胶囊：窗口宽度按波形条数固定，所以这里必须禁止换行 —— 否则文字一多
              （如「剩余 28s」+「请靠近麦克风」同时出现）就会折成两行，把圆角撑破。
              超出时让**波形**让位（见下方 bars 容器的 min-w-0 + overflow-hidden），
              文字保持完整：文字是信息，波形只是陪衬。 */}
          {/* max-w 是配套的另一半：根容器是 justify-center，胶囊内容一宽就向两侧溢出、
              被窗口裁掉，看着像"两端被切平的圆角矩形"。给了宽度上限，压缩压力才会
              传到波形上（bars 的 min-w-0），胶囊本身始终保持完整的胶囊形状。
              用 100vw 而不是 max-w-full：父层宽度也是按内容算的，撑不出约束。 */}
          <div
            className="flex max-w-[calc(100vw-8px)] items-center overflow-hidden whitespace-nowrap rounded-full border px-4 py-2"
            style={{
              background: 'var(--overlay-bg)',
              color: 'var(--overlay-text)',
              borderColor: 'var(--overlay-border)',
            }}
          >
            {state === 'waiting' && (
              <div className="flex items-center gap-[3px]" style={{ height: '20px' }}>
                {[0, 1, 2].map((i) => (
                  <div
                    key={i}
                    className="h-[3px] w-[3px] rounded-full"
                    style={{
                      backgroundColor: 'var(--overlay-text-dim)',
                      animation: `dot-pulse 1s ease-in-out ${i * 0.15}s infinite`,
                    }}
                  />
                ))}
              </div>
            )}

            {state === 'listening' && (
              warning && warningTone === 'error' ? (
                // 静音高警：波形此时是平的、无意义，直接在胶囊里居中显示红字，
                // 既更醒目、也避免波形+长文字撑破固定宽度把胶囊圆角裁掉。
                <div
                  className="flex items-center whitespace-nowrap px-1 text-xs font-semibold text-red-500 animate-pulse"
                  style={{ height: '20px' }}
                >
                  {warning}
                </div>
              ) : (
                <>
                  {/* 有文字要占位时**少画几根**，而不是让 overflow 把波形裁一半 —— 裁出来的
                      半根条子看着像坏了。少画是"看起来就是这么设计的"。
                      min-w-0 仍然留着兜底：万一文字特别长，宁可裁波形也不折行。 */}
                  <div className="flex min-w-0 items-center gap-[2px] overflow-hidden" style={{ height: '20px' }}>
                    {visibleBars.map((height, index) => {
                      const color = getListeningBarColor(index, bars.length, theme)
                      return (
                        <div
                          key={index}
                          className="w-[2.5px] rounded-full"
                          style={{
                            backgroundColor: color,
                            boxShadow: 'none',
                            height: `${Math.min(18, Math.max(3, height))}px`,
                            opacity: 0.7 + (Math.min(18, height) / 18) * 0.3,
                            transition: 'height 50ms ease-out, opacity 50ms ease-out',
                          }}
                        />
                      )
                    })}
                  </div>
                  {/* 计时**不再**被警告顶掉：警告是一闪而过的提示，计时是持续状态。
                      以前条件里带 !warning，导致 4 分钟提示一出现，剩下一整分钟都看不到秒数。 */}
                  {showDuration && (
                    <span
                      className={`ml-1.5 shrink-0 whitespace-nowrap text-right font-mono tabular-nums text-xs${inCountdown ? ' font-semibold' : ''}${urgent ? ' animate-pulse' : ''}`}
                      style={{ color: timerColor }}
                    >
                      {timerText}
                    </span>
                  )}
                  {warning && (
                    <span className="ml-2 shrink-0 whitespace-nowrap text-xs text-amber-400 animate-pulse">
                      {warning}
                    </span>
                  )}
                </>
              )
            )}

            {state === 'thinking' && (
              <div className="flex items-center gap-2">
                <div className="relative h-1 w-12 overflow-hidden rounded-full bg-white/10">
                  <div
                    className="absolute left-0 top-0 h-full rounded-full"
                    style={{
                      backgroundColor: thinkingColor,
                      width: '100%',
                      transformOrigin: 'left',
                      animation: `progress-fill ${thinkingDuration}s cubic-bezier(0.4, 0, 0.2, 1) forwards`,
                    }}
                  />
                </div>
                <span className="text-xs whitespace-nowrap" style={{ color: thinkingColor }}>{t('overlay.processing')}</span>
                {/* 这里**故意**不写「Esc 取消」。Esc 取消的能力照常生效（原生钩子 +
                    escape-action，与本组件无关），只是这行提示不该出现在悬浮窗上：
                    悬浮窗是贴在光标附近、每次口述都会闪一下的东西，越安静越好，而
                    「按 Esc 能取消」是知道一次就一直知道的事实，不需要每次复述。
                    这件事已在 设置 → 键盘快捷键 的说明里静态交代（见 GeneralSettingsPage）。
                    要加回来之前先想清楚：它每次处理都出现，收益只有第一次。 */}
              </div>
            )}

            {state === 'error' && (
              <div className="flex items-center gap-2">
                <span className="text-xs text-red-400">{errorMessage || t('overlay.genericError')}</span>
              </div>
            )}

            {state === 'toast' && (
              <div className="flex items-center gap-2">
                {toastTone === 'warn' ? (
                  <span className="whitespace-nowrap text-xs text-amber-400">{toastText}</span>
                ) : (
                  <span className="whitespace-nowrap text-xs" style={{ color: 'var(--overlay-text)' }}>
                    {toastText}
                  </span>
                )}
              </div>
            )}
          </div>
        </div>
      )}
    </div>
  )
}
