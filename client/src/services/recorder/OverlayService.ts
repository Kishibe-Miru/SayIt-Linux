import { getLocale, t } from '@/i18n'
import * as bridge from '../bridge'
import { addRuntimeEvent } from '../debugLog'
import { getSetting } from '../store'
import { clampSec } from '../timeModel'
import {
  formatRecordingLimit,
  OVERLAY_WIDTH_PRESETS,
  type OverlayCommonPayload,
  type OverlayWaveTheme,
  type OverlayWidthPreset,
} from './types'

function normalizeTheme(value: unknown): OverlayWaveTheme {
  if (value === 'black-white' || value === 'black-blue' || value === 'black-rainbow') {
    return value
  }
  return 'black-white'
}

function normalizeWidthPreset(value: unknown): OverlayWidthPreset {
  if (value === 'short' || value === 'medium' || value === 'long') return value
  return 'long'
}

export class OverlayService {
  private theme: OverlayWaveTheme = 'black-white'
  private showDuration = true
  private readySoundEnabled = true
  private widthPreset: OverlayWidthPreset = 'medium'
  private lastFrameAt = 0
  private tickerId: ReturnType<typeof setInterval> | null = null
  private fallbackHideId: ReturnType<typeof setTimeout> | null = null
  /** Persistent warning text — included in every overlay update until cleared */
  private activeWarning = ''
  private timeoutWarningHideId: ReturnType<typeof setTimeout> | null = null
  /** 流式实时识别文本 — 录音期间随中间结果更新，会被并入每次 listening 更新一起下发 */
  private streamingText = ''
  /** 本次录音是否开启流式实时显示：为真则从录音一开始就显示气泡（占位），中途不再缩放窗口 */
  private streamingActive = false

  constructor(private readonly getElapsedSec: () => number) { }

  async refreshSettings() {
    this.theme = normalizeTheme(await getSetting('overlayWaveTheme', 'black-white'))
    this.showDuration = Boolean(await getSetting('overlayShowDuration', true))
    this.readySoundEnabled = Boolean(await getSetting('readySoundEnabled', true))
    this.widthPreset = normalizeWidthPreset(await getSetting('overlayWidth', 'medium'))
  }

  private readySoundCtx: AudioContext | null = null

  private playReadySound() {
    if (!this.readySoundEnabled) return
    try {
      if (!this.readySoundCtx || this.readySoundCtx.state === 'closed') {
        this.readySoundCtx = new AudioContext()
      }
      const ctx = this.readySoundCtx
      // AudioContext 新建或长时间无操作后常处于 suspended 状态（浏览器自动播放/省电策略）。
      // resume() 是异步的，若不等它完成就 start()，振荡器可能被静默丢弃或延迟——
      // 这正是「第一次按免提键听不到提示音，过一会又听不到」的根因。
      if (ctx.state === 'suspended') {
        ctx.resume().then(() => this.emitReadyTone(ctx)).catch(() => { /* ignore */ })
      } else {
        this.emitReadyTone(ctx)
      }
    } catch { /* ignore */ }
  }

  private emitReadyTone(ctx: AudioContext) {
    try {
      const osc = ctx.createOscillator()
      const gain = ctx.createGain()
      osc.connect(gain)
      gain.connect(ctx.destination)
      osc.frequency.value = 880
      osc.type = 'sine'
      gain.gain.setValueAtTime(0.2, ctx.currentTime)
      gain.gain.exponentialRampToValueAtTime(0.001, ctx.currentTime + 0.15)
      osc.start(ctx.currentTime)
      osc.stop(ctx.currentTime + 0.15)
    } catch { /* ignore */ }
  }

  getCommonPayload(): OverlayCommonPayload {
    const cfg = OVERLAY_WIDTH_PRESETS[this.widthPreset]
    return {
      theme: this.theme,
      showDuration: this.showDuration,
      baseWidth: cfg.windowWidth,
      barCount: cfg.barCount,
      // 每次都带上：悬浮窗可能在语言切换之后才第一次创建，没有"只发一次"的时机。
      locale: getLocale(),
    }
  }

  getBarCount(): number {
    return OVERLAY_WIDTH_PRESETS[this.widthPreset].barCount
  }

  private setEscapeMode(mode: bridge.EscapeActionMode, token = 0) {
    void bridge.setEscapeActionMode(mode, token).catch(() => { /* 原生钩子不可用时不影响悬浮窗 */ })
  }

  /** 文本即将进入不可逆的系统粘贴阶段；先关闭全局 Esc 取消，避免“已取消”后仍完成粘贴。 */
  async disableEscapeAction(): Promise<void> {
    try {
      await bridge.setEscapeActionMode('off', 0)
    } catch { /* 原生钩子不可用时不影响文本插入 */ }
  }

  showWaiting() {
    this.setEscapeMode('off', 0)
    this.clearFallbackHideTimer()
    void bridge.presentOverlay({
      state: 'waiting',
      elapsedSec: 0,
      ...this.getCommonPayload(),
    })
  }

  startListeningTicker(token = 0) {
    this.stopListeningTicker()
    // 真实录音代次才开启全局 Esc；PTT Lab 等 token=0 的预览绝不吞系统按键。
    this.setEscapeMode(token > 0 ? 'cancel_recording' : 'off', token)
    this.playReadySound()
    this.tickerId = setInterval(() => {
      void bridge.updateOverlay({
        state: 'listening',
        elapsedSec: clampSec(this.getElapsedSec()),
        ...(this.activeWarning ? { warning: this.activeWarning } : {}),
        ...(this.streamingActive ? { streaming: true } : {}),
        ...(this.streamingText ? { streamingText: this.streamingText } : {}),
        ...this.getCommonPayload(),
      })
    }, 33)
  }

  /** 设置本次录音是否开启流式实时显示（录音开始时调用）。开启后气泡从一开始就显示占位。 */
  setStreamingActive(on: boolean) {
    this.streamingActive = on
  }

  /** 更新流式实时识别文本。下一次 listening 心跳（33ms）会把它带给悬浮窗一起渲染。
   *  传空字符串可清除气泡。 */
  setStreamingText(text: string) {
    this.streamingText = text || ''
  }

  /** 清除流式实时文本与激活标志（录音停止 / 重置时调用）。 */
  resetStreamingText() {
    this.streamingText = ''
    this.streamingActive = false
  }

  stopListeningTicker() {
    if (this.tickerId) {
      clearInterval(this.tickerId)
      this.tickerId = null
    }
  }

  pushListeningBars(bars?: number[], force = false) {
    const now = Date.now()
    if (!force && now - this.lastFrameAt < 33) return
    this.lastFrameAt = now
    void bridge.updateOverlay({
      state: 'listening',
      bars,
      elapsedSec: clampSec(this.getElapsedSec()),
      ...(this.activeWarning ? { warning: this.activeWarning } : {}),
      ...(this.streamingActive ? { streaming: true } : {}),
      ...(this.streamingText ? { streamingText: this.streamingText } : {}),
      ...this.getCommonPayload(),
    })
  }

  showThinking(elapsedSec: number, token = 0) {
    // PTT Lab 也复用 thinking 动画，但没有真实录音代次；token=0 时只显示，绝不吞全局 Esc。
    this.setEscapeMode(token > 0 ? 'cancel_processing' : 'off', token)
    void bridge.updateOverlay({
      state: 'thinking',
      elapsedSec: clampSec(elapsedSec),
      ...this.getCommonPayload(),
    })
  }

  /**
   * 即将到达单次上限的提示。**一闪即走**，之后由计时区的倒计时持续告知剩余时间。
   *
   * 以前这条是常驻到录音结束的，加上 Overlay 里"有警告就不显示计时"的条件，
   * 导致最后一整分钟只剩一句「单次记录最长300s」，既不知道已录多久、也不知道还剩多久。
   */
  showTimeoutWarning() {
    const text = t('overlay.warnMaxDuration', { limit: formatRecordingLimit() })
    this.activeWarning = text
    void bridge.updateOverlay({
      state: 'listening',
      warning: text,
      warningTone: 'warn',
      elapsedSec: clampSec(this.getElapsedSec()),
      ...this.getCommonPayload(),
    })

    if (this.timeoutWarningHideId) clearTimeout(this.timeoutWarningHideId)
    this.timeoutWarningHideId = setTimeout(() => {
      this.timeoutWarningHideId = null
      // 期间若被更高优先级的警告（如麦克风被静音）取代，就不要抢回来
      if (this.activeWarning !== text) return
      this.activeWarning = ''
      void bridge.updateOverlay({
        state: 'listening',
        warning: '',
        warningTone: 'warn',
        elapsedSec: clampSec(this.getElapsedSec()),
        ...this.getCommonPayload(),
      })
    }, 4000)
  }

  /** Show low volume warning on the overlay（有声音但偏低：请靠近麦克风，琥珀色） */
  showLowVolumeWarning() {
    if (this.activeWarning) return
    void bridge.updateOverlay({
      state: 'listening',
      warning: t('overlay.warnLowVolume'),
      warningTone: 'warn',
      elapsedSec: clampSec(this.getElapsedSec()),
      ...this.getCommonPayload(),
    })
  }

  /** Show "no signal detected" warning（几乎无信号但未能确认被静音：选错设备/未授权等，琥珀色） */
  showNoSignalWarning() {
    if (this.activeWarning) return
    void bridge.updateOverlay({
      state: 'listening',
      warning: t('overlay.warnNoSignal'),
      warningTone: 'warn',
      elapsedSec: clampSec(this.getElapsedSec()),
      ...this.getCommonPayload(),
    })
  }

  /** Show "mic is muted" high-alert（系统层面确认被静音：红色高警） */
  showMicMutedAlert() {
    if (this.activeWarning) return
    void bridge.updateOverlay({
      state: 'listening',
      warning: t('overlay.warnMicMuted'),
      warningTone: 'error',
      elapsedSec: clampSec(this.getElapsedSec()),
      ...this.getCommonPayload(),
    })
  }

  /** Clear transient warnings (low volume etc.) — does NOT clear timeout warning */
  clearWarning() {
    if (this.activeWarning) return
    void bridge.updateOverlay({
      state: 'listening',
      warning: '',
      warningTone: 'warn',
      elapsedSec: clampSec(this.getElapsedSec()),
      ...this.getCommonPayload(),
    })
  }

  /**
   * 是否有常驻（sticky）警告正在挡住临时警告的显示/清除。
   * 排查"低音量提示不消失/不出现"这类问题时很有用：那类症状多半就是被它挡着。
   */
  hasStickyWarning() {
    return !!this.activeWarning
  }

  /** Reset all warnings including persistent ones (called on recording stop/reset) */
  resetWarnings() {
    if (this.timeoutWarningHideId) {
      clearTimeout(this.timeoutWarningHideId)
      this.timeoutWarningHideId = null
    }
    this.activeWarning = ''
  }

  showFallback(text: string, reason: string, token = 0) {
    console.log('[OverlayService] showFallback called, text:', text.slice(0, 30), 'reason:', reason)
    // token=0 的 PTT Lab 卡片没有 Orchestrator 代次，不能开启一个无人消费的全局 Esc。
    this.setEscapeMode(token > 0 ? 'dismiss_fallback' : 'off', token)
    void bridge.presentOverlay({
      state: 'fallback',
      fallbackText: text,
      fallbackReason: reason,
      ...this.getCommonPayload(),
    })
    this.clearFallbackHideTimer()
    this.fallbackHideId = setTimeout(() => {
      console.log('[OverlayService] fallback auto-hide timer fired')
      this.hide()
    }, 10000)
  }

  clearFallbackHideTimer() {
    if (this.fallbackHideId) {
      clearTimeout(this.fallbackHideId)
      this.fallbackHideId = null
    }
  }

  hide() {
    this.clearFallbackHideTimer()
    this.setEscapeMode('off', 0)
    void bridge.hideOverlay()
  }

  /** 快捷键切换润色模式后，用悬浮窗短暂提示当前模式名，约 1.6s 后自动隐藏。 */
  showPresetSwitched(name: string) {
    this.setEscapeMode('off', 0)
    void bridge.presentOverlay({
      state: 'toast',
      toastText: t('overlay.toastPresetSwitched', { name }),
      toastTone: 'info',
      ...this.getCommonPayload(),
    })
    this.clearFallbackHideTimer()
    this.fallbackHideId = setTimeout(() => this.hide(), 1600)
  }

  /**
   * 识别结果为空（未检测到有效声音）时的提示。
   *
   * `diagnostic` 由调用方补充成因上下文。日志刻意放在这里、而不是各个调用点：
   * 这句提示是全软件最容易被误读的一条 —— 真实成因可能是用户没说话、供应商额度
   * 耗尽、服务端提前断开、文本后处理失败，用户看到的却是同一句话。放在这个唯一
   * 出口，就保证「用户看到这句」和「日志里有记录」永远一一对应，不会因为将来
   * 新增一个调用点而漏掉。
   */
  showNoSpeech(diagnostic?: Record<string, unknown>) {
    addRuntimeEvent('warn', 'recorder', 'Showing no-speech warning', diagnostic ?? {})
    this.setEscapeMode('off', 0)
    void bridge.presentOverlay({
      state: 'toast',
      toastText: t('overlay.toastNoSpeech'),
      toastTone: 'warn',
      ...this.getCommonPayload(),
    })
    this.clearFallbackHideTimer()
    this.fallbackHideId = setTimeout(() => this.hide(), 1500)
  }

  /** 用户主动取消处理后的短提示。 */
  showCanceled() {
    this.setEscapeMode('off', 0)
    void bridge.presentOverlay({
      state: 'toast',
      toastText: t('overlay.toastCanceled'),
      toastTone: 'info',
      ...this.getCommonPayload(),
    })
    this.clearFallbackHideTimer()
    this.fallbackHideId = setTimeout(() => this.hide(), 900)
  }

  /** 显示错误信息，几秒后自动隐藏 */
  showError(message: string) {
    this.setEscapeMode('off', 0)
    void bridge.presentOverlay({
      state: 'error',
      errorMessage: message,
      ...this.getCommonPayload(),
    })
    this.clearFallbackHideTimer()
    this.fallbackHideId = setTimeout(() => this.hide(), 4000)
  }

  dispose() {
    this.stopListeningTicker()
    this.resetStreamingText()
    this.hide()
  }
}
