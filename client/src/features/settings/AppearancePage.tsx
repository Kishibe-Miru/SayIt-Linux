// 外观设置页面 — 主题 + 悬浮窗样式 + 预览

import { useEffect, useRef, useState } from 'react'
import { open as shellOpen } from '@tauri-apps/plugin-shell'
import { ExternalLink, Info } from 'lucide-react'
import { Tooltip } from '@/components/ui/tooltip'
import { Card, CardContent } from '@/components/ui/card'
import { Switch } from '@/components/ui/switch'
import { themeList } from '@/themes'
import { switchTheme, getActiveThemeId } from '@/stores/theme'
import { getSetting, setSetting } from '@/services/store'
import { refreshOverlaySettings, setStreamingDisplayCache } from '@/services/recorder'
import { OVERLAY_WIDTH_PRESETS, type OverlayWidthPreset } from '@/services/recorder/types'
import { type OverlayWaveTheme } from './utils'
import { type TranslationKey } from '@/i18n'
import { useT } from '@/i18n/useT'

const OVERLAY_OPTIONS: Array<{
  theme: OverlayWaveTheme
  labelKey: TranslationKey
  barColors: string[]
}> = [
    { theme: 'black-white', labelKey: 'appearance.wave.blackWhite', barColors: ['#e2e8f0', '#cbd5e1', '#94a3b8'] },
    { theme: 'black-blue', labelKey: 'appearance.wave.blackBlue', barColors: ['#22d3ee', '#3b82f6', '#6366f1'] },
    { theme: 'black-rainbow', labelKey: 'appearance.wave.blackRainbow', barColors: ['#4ade80', '#facc15', '#fb923c', '#f87171'] },
  ]

// 由长到短排列：从「最完整」往「最克制」读，比反过来更符合挑尺寸的直觉。
const WIDTH_OPTIONS: Array<{ value: OverlayWidthPreset; labelKey: TranslationKey }> = [
  { value: 'long', labelKey: 'appearance.width.long' },
  { value: 'medium', labelKey: 'appearance.width.medium' },
  { value: 'short', labelKey: 'appearance.width.short' },
]

function getBarColor(index: number, total: number, theme: OverlayWaveTheme): string {
  const t = index / Math.max(1, total - 1)
  if (theme === 'black-white') return '#f1f5f9'
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



function OverlayPreview({ theme, showDuration, barCount, streaming }: { theme: OverlayWaveTheme; showDuration: boolean; barCount: number; streaming: boolean }) {
  const t = useT()
  const previewText = t('appearance.streamingPreviewText')
  const barRefs = useRef<Array<HTMLDivElement | null>>([])
  const [typed, setTyped] = useState('')

  // 流式预览：循环把示例文字一个字一个字打出来，模拟"边说边出字"的动态效果
  useEffect(() => {
    if (!streaming) {
      setTyped('')
      return
    }
    let i = 0
    let timer: ReturnType<typeof setTimeout>
    const step = () => {
      if (i <= previewText.length) {
        setTyped(previewText.slice(0, i))
        i += 1
        timer = setTimeout(step, 130)
      } else {
        // 打完停顿一下再从头循环
        timer = setTimeout(() => { i = 0; step() }, 1600)
      }
    }
    step()
    return () => clearTimeout(timer)
  }, [streaming, previewText])

  useEffect(() => {
    const heights = new Array(barCount).fill(3)
    let running = true
    let rafId = 0
    let lastFrame = 0
    const FRAME_INTERVAL = 1000 / 30 // 30fps 足够流畅，且不占满主线程

    const animate = (now: number) => {
      if (!running) return
      if (now - lastFrame >= FRAME_INTERVAL) {
        lastFrame = now
        for (let i = 0; i < heights.length; i++) {
          const target = 3 + Math.random() * 15
          heights[i] = heights[i] + (target - heights[i]) * 0.15
          const el = barRefs.current[i]
          if (el) {
            const h = Math.min(18, Math.max(3, heights[i]))
            el.style.height = `${h}px`
            el.style.opacity = String(0.7 + (h / 18) * 0.3)
          }
        }
      }
      rafId = requestAnimationFrame(animate)
    }
    rafId = requestAnimationFrame(animate)
    return () => {
      running = false
      cancelAnimationFrame(rafId)
    }
  }, [barCount])

  return (
    <div className="flex flex-col items-center gap-3">
      {/* 流式实时字幕气泡（开启时显示，带打字动画） */}
      {streaming && (
        <div className="relative w-[260px] rounded-2xl border border-slate-600 bg-black px-3.5 py-2.5 shadow-[0_6px_16px_rgba(0,0,0,0.35)]">
          <span className="mb-1 block text-[10px] font-medium tracking-[0.18em] text-slate-400">{t('appearance.streamingPreviewLabel')}</span>
          {/* 内容驱动、底部对齐，和真实悬浮窗一致 */}
          <div className="flex max-h-[40px] flex-col justify-end overflow-hidden text-left text-[13px] leading-5 text-slate-100">
            <div>
              {typed}
              <span
                className="ml-0.5 inline-block h-[1.05em] w-[2px] rounded-full align-middle"
                style={{ backgroundColor: '#f1f5f9', animation: 'caret-blink 1.1s ease-in-out infinite' }}
              />
            </div>
          </div>
          {/* 朝下尖角，指向下方录音胶囊 */}
          <span
            className="absolute left-1/2 -translate-x-1/2"
            style={{ bottom: '-7px', width: 0, height: 0, borderLeft: '7px solid transparent', borderRight: '7px solid transparent', borderTop: '7px solid #000' }}
          />
        </div>
      )}
      {/* 1:1 还原真实悬浮窗样式 */}
      <div className="flex items-center rounded-full border border-slate-600 bg-black px-4 py-2 shadow-[0_6px_16px_rgba(0,0,0,0.35)]">
        <div className="flex items-center gap-[2px]" style={{ height: '20px' }}>
          {Array.from({ length: barCount }, (_, index) => {
            const color = getBarColor(index, barCount, theme)
            return (
              <div
                key={index}
                ref={(el) => { barRefs.current[index] = el }}
                className="w-[2.5px] rounded-full"
                style={{
                  backgroundColor: color,
                  height: '3px',
                  opacity: 0.7,
                  transition: 'height 50ms ease-out, opacity 50ms ease-out',
                }}
              />
            )
          })}
        </div>
        {showDuration && (
          <span
            className="ml-1.5 min-w-[24px] text-right font-mono tabular-nums text-xs"
            style={{ color: getTimerColor(theme) }}
          >
            3.2s
          </span>
        )}
      </div>
      <span className="text-xs text-muted-foreground">{t('appearance.overlayPreview')}</span>
    </div>
  )
}

export default function AppearancePage() {
  const t = useT()
  const [activeTheme, setActiveTheme] = useState(getActiveThemeId)
  const [overlayWaveTheme, setOverlayWaveTheme] = useState<OverlayWaveTheme>('black-white')
  const [overlayShowDuration, setOverlayShowDuration] = useState(true)
  const [overlayWidth, setOverlayWidth] = useState<OverlayWidthPreset>('medium')
  const [streamingDisplay, setStreamingDisplay] = useState(false)
  // 读到已保存值之前，控件先隐藏、且不带过渡：避免「默认值 → 已保存值」闪一下。
  // 注意过渡也必须一起关掉 —— visibility:hidden 只是看不见，CSS 过渡照样会跑；
  // 隐藏期间选中项仍是默认值，揭开的同一刻颜色会从旧值过渡到新值，看起来就是
  // 「默认项高亮淡出、已保存项淡入」（悬浮窗长度从「中」闪到「短」正是如此）。
  // 隐藏一律用内联 style，不用 Tailwind 的 invisible 类：类要靠扫描源码产出，
  // 一旦跑的是旧的 CSS 产物这层保护就整条失效（曾因此反复排查很久）。
  const [ready, setReady] = useState(false)
  // animate 与 ready 必须分开：
  // 按 CSS 过渡规范，浏览器看的是「变化之后」的样式里有没有 transition。若在揭开、
  // 赋值的同一帧把 transition 加上，浏览器就会认为「有过渡且颜色变了」，于是把
  // 「默认项高亮淡出、已保存项淡入」真的动画一遍 —— 那就是反复没修掉的那个「闪」。
  // 因此：揭开的那一帧仍然不带过渡，等再下一帧才允许过渡（此时颜色已无变化）。
  const [animate, setAnimate] = useState(false)

  useEffect(() => {
    // 先把所有值取回，再在同一个同步块里一次性落值 + 置 ready：React 会把这批更新
    // 合成一次渲染，因此不存在「已显示但值还没到」的中间态（那正是闪一下的成因）。
    //
    // 两个坑都在这里躲掉了：
    //  1. 每项自带 catch 兜底 —— Promise.all 是 fail-fast，只要有一项 reject 就会
    //     立刻往下走，那时其它项还没回来，ready 会在值到位前就被置上（并留下一个
    //     未处理的 rejection）。
    //  2. 不用 requestAnimationFrame —— 那会把 setReady 丢到与赋值不同的批次里，
    //     同样可能先提交出一帧「已显示但值是默认值」。
    let cancelled = false
    void (async () => {
      const [showDuration, streaming, waveTheme, width] = await Promise.all([
        getSetting('overlayShowDuration', true).catch(() => true),
        getSetting('streamingDisplayEnabled', false).catch(() => false),
        getSetting('overlayWaveTheme', 'black-white').catch(() => 'black-white'),
        getSetting('overlayWidth', 'medium').catch(() => 'medium'),
      ])
      if (cancelled) return
      setOverlayShowDuration(Boolean(showDuration))
      setStreamingDisplay(Boolean(streaming))
      const t = waveTheme as OverlayWaveTheme
      if (t === 'black-white' || t === 'black-blue' || t === 'black-rainbow') setOverlayWaveTheme(t)
      const w = width as OverlayWidthPreset
      if (w === 'short' || w === 'medium' || w === 'long') setOverlayWidth(w)
      setReady(true)
      requestAnimationFrame(() => requestAnimationFrame(() => {
        if (!cancelled) setAnimate(true)
      }))
    })()
    return () => { cancelled = true }
  }, [])

  const handleThemeChange = async (themeId: string) => {
    await switchTheme(themeId)
    setActiveTheme(themeId)
  }

  const handleOverlayThemeChange = async (theme: OverlayWaveTheme) => {
    setOverlayWaveTheme(theme)
    await setSetting('overlayWaveTheme', theme)
    await refreshOverlaySettings()
  }

  const handleToggleDuration = async () => {
    const next = !overlayShowDuration
    setOverlayShowDuration(next)
    await setSetting('overlayShowDuration', next)
    await refreshOverlaySettings()
  }

  const handleOverlayWidthChange = async (preset: OverlayWidthPreset) => {
    setOverlayWidth(preset)
    await setSetting('overlayWidth', preset)
    await refreshOverlaySettings()
  }

  const handleToggleStreamingDisplay = () => {
    const next = !streamingDisplay
    setStreamingDisplay(next)
    setStreamingDisplayCache(next) // 立即同步录音器缓存，无需重启即可生效
    void setSetting('streamingDisplayEnabled', next)
  }

  return (
    <div className="mx-auto max-w-4xl p-8">
      <h1 className="mb-6 text-2xl font-bold">{t('appearance.title')}</h1>

      <div className="space-y-6">
        <Card>
          <CardContent className="p-6">
            <h2 className="mb-4 text-lg font-semibold">{t('appearance.appTheme')}</h2>
            <div className="grid gap-3 sm:grid-cols-3">
              {themeList.map((theme) => (
                <button
                  key={theme.id}
                  type="button"
                  onClick={() => void handleThemeChange(theme.id)}
                  className={`flex items-center gap-3 rounded-lg border p-3 transition-colors ${activeTheme === theme.id
                    ? 'border-primary bg-primary/5'
                    : 'border-border hover:bg-accent'
                    }`}
                >
                  <div className="flex gap-1">
                    {Object.values(theme.previewColors).map((color, i) => (
                      <span
                        key={i}
                        className="h-5 w-5 rounded-full border border-border/50"
                        style={{ backgroundColor: color }}
                      />
                    ))}
                  </div>
                  <span className="text-sm font-medium">{theme.name}</span>
                </button>
              ))}
            </div>
          </CardContent>
        </Card>

        <Card>
          <CardContent className="p-6">
            <h2 className="mb-4 text-lg font-semibold">{t('appearance.overlayStyle')}</h2>

            {/* 整块一起等值到位再显示。除了选择器，**悬浮窗预览**那颗胶囊的长度也跟着
                overlayWidth 变（它有可见边框），只盖住选择器的话，仍会看到预览从「中」
                的长度一下缩到「短」——那正是之前反复没修掉的那个「闪」。 */}
            <div className="space-y-4" style={ready ? undefined : { visibility: 'hidden' }}>
              <div>
                <p className="mb-2 text-sm text-muted-foreground">{t('appearance.waveTheme')}</p>
                <div className="grid gap-2 sm:grid-cols-3" style={ready ? undefined : { visibility: 'hidden' }}>
                  {OVERLAY_OPTIONS.map((option) => (
                    <button
                      key={option.theme}
                      type="button"
                      onClick={() => void handleOverlayThemeChange(option.theme)}
                      className={`flex items-center justify-between rounded-md border px-3 py-2 text-sm ${animate ? 'transition-colors' : ''} ${overlayWaveTheme === option.theme
                        ? 'border-primary bg-primary/5'
                        : 'border-border hover:bg-accent'
                        }`}
                    >
                      <span className="flex items-center gap-2">
                        <span className={`flex h-3.5 w-3.5 items-center justify-center rounded-full border ${overlayWaveTheme === option.theme ? 'border-primary' : 'border-muted-foreground/40'}`}>
                          {overlayWaveTheme === option.theme && <span className="h-2 w-2 rounded-full bg-primary" />}
                        </span>
                        <span>{t(option.labelKey)}</span>
                      </span>
                      <span className="flex gap-0.5">
                        {option.barColors.map((c, i) => (
                          <span key={i} className="h-3 w-1 rounded-sm" style={{ backgroundColor: c }} />
                        ))}
                      </span>
                    </button>
                  ))}
                </div>

                <div className="mt-4">
                  <p className="mb-2 text-sm text-muted-foreground">{t('appearance.overlayWidth')}</p>
                  <div className="grid gap-2 sm:grid-cols-3" style={ready ? undefined : { visibility: 'hidden' }}>
                    {WIDTH_OPTIONS.map((option) => (
                      <button
                        key={option.value}
                        type="button"
                        onClick={() => void handleOverlayWidthChange(option.value)}
                        className={`flex items-center gap-2 rounded-md border px-3 py-2 text-sm ${animate ? 'transition-colors' : ''} ${overlayWidth === option.value
                          ? 'border-primary bg-primary/5'
                          : 'border-border hover:bg-accent'
                          }`}
                      >
                        <span className={`flex h-3.5 w-3.5 items-center justify-center rounded-full border ${overlayWidth === option.value ? 'border-primary' : 'border-muted-foreground/40'}`}>
                          {overlayWidth === option.value && <span className="h-2 w-2 rounded-full bg-primary" />}
                        </span>
                        <span>{t(option.labelKey)}</span>
                      </button>
                    ))}
                  </div>
                </div>

                <div className="mt-4 flex items-center justify-between">
                  <div>
                    <p className="text-sm font-medium">{t('appearance.showDuration')}</p>
                    <p className="text-xs text-muted-foreground">{t('appearance.showDurationDesc')}</p>
                  </div>
                  <Switch checked={overlayShowDuration} onChange={handleToggleDuration} noAnimation={!animate} hidden={!ready} />
                </div>

                <div className="mt-4 flex items-center justify-between">
                  <div className="pr-3">
                    <div className="flex items-center gap-1.5">
                      <p className="text-sm font-medium">{t('appearance.streaming')}</p>
                      <Tooltip
                        variant="light"
                        content={t('appearance.streamingHelp')}
                      >
                        <Info className="h-3.5 w-3.5 shrink-0 cursor-help text-muted-foreground/50 transition-colors hover:text-muted-foreground" />
                      </Tooltip>
                    </div>
                    <p className="text-xs text-muted-foreground">
                      {t('appearance.streamingDesc')}
                      <span className="text-muted-foreground/70">
                        {t('appearance.streamingNote')}
                      </span>
                      <button
                        type="button"
                        onClick={() => void shellOpen('https://console.volcengine.com/speech/new/setting/activate?projectName=default')}
                        className="inline-flex items-center gap-0.5 text-primary underline underline-offset-2 decoration-primary/50 transition-colors hover:decoration-primary"
                      >
                        {t('appearance.streamingActivate')}
                        <ExternalLink className="h-3 w-3" />
                      </button>
                      <span className="text-muted-foreground/70">{t('appearance.streamingNoteEnd')}</span>
                    </p>
                  </div>
                  <Switch checked={streamingDisplay} onChange={handleToggleStreamingDisplay} noAnimation={!animate} hidden={!ready} />
                </div>

                <div className="mt-4 flex justify-center">
                  <OverlayPreview theme={overlayWaveTheme} showDuration={overlayShowDuration} barCount={OVERLAY_WIDTH_PRESETS[overlayWidth].barCount} streaming={streamingDisplay} />
                </div>
              </div>
            </div>
          </CardContent>
        </Card>
      </div>
    </div>
  )
}
