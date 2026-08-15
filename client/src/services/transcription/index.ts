// Provider 管理器 — 根据 workMode 返回对应的 TranscriptionProvider

import { invoke } from '@tauri-apps/api/core'
import { getSetting } from '../store'
import { addRuntimeEvent } from '../debugLog'
import { ServerProvider } from './ServerProvider'
import { CloudAPIProvider } from './CloudAPIProvider'
import { LocalProvider } from './LocalProvider'
import type { TranscriptionProvider, WorkMode } from './types'

export type { TranscriptionProvider, TranscriptionCallbacks, StartOptions, StopOptions, FinalResult, ASRResult, WorkMode, ProviderState } from './types'

let currentProvider: TranscriptionProvider | null = null
let currentMode: WorkMode = 'local'

function createProvider(mode: WorkMode): TranscriptionProvider {
  switch (mode) {
    case 'server':
      return new ServerProvider()
    case 'cloud_api':
      return new CloudAPIProvider()
    case 'local':
      return new LocalProvider()
    default:
      addRuntimeEvent('warn', 'transcription', `Unknown processing mode "${mode}"; falling back to local mode`)
      return new LocalProvider()
  }
}

/** 获取当前 Provider 实例（懒初始化） */
export function getProvider(): TranscriptionProvider {
  if (!currentProvider) {
    currentProvider = createProvider(currentMode)
  }
  return currentProvider
}

/** 获取当前工作模式 */
export function getWorkMode(): WorkMode {
  return currentMode
}

/**
 * 切换工作模式。
 * 会断开旧 Provider 并创建新的。
 * 调用方需要重新 connect。
 */
export async function switchProvider(mode: WorkMode): Promise<TranscriptionProvider> {
  if (mode === currentMode && currentProvider) {
    return currentProvider
  }

  addRuntimeEvent('info', 'transcription', 'Processing mode changed', { from: currentMode, to: mode })

  // 断开旧 Provider
  if (currentProvider) {
    try {
      currentProvider.disconnect()
    } catch {
      // ignore
    }
  }

  // 离开本地模式时释放 sherpa-onnx recognizer 占用的内存（几百 MB ~ 数 GB），
  // 否则切到云 API / 服务器模式后本地模型仍常驻到应用退出。
  if (currentMode === 'local' && mode !== 'local') {
    try {
      await invoke('unload_local_model')
    } catch (err) {
      addRuntimeEvent('warn', 'transcription', 'Failed to release local model', { error: String(err) })
    }
  }

  currentMode = mode
  currentProvider = createProvider(mode)
  return currentProvider
}

/** 从 store 读取保存的 workMode 并初始化 */
export async function initProviderFromStore(): Promise<void> {
  const stored = await getSetting('workMode', 'local')
  const mode = (stored === 'server' || stored === 'cloud_api' || stored === 'local') ? stored : 'local'
  currentMode = mode as WorkMode
  currentProvider = createProvider(currentMode)
  addRuntimeEvent('info', 'transcription', 'Provider initialized', { mode: currentMode })
}
