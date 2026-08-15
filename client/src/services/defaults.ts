/**
 * 集中式默认配置
 *
 * 所有 getSetting() 的默认值统一在此定义。
 * 修改默认值只需改这一个文件，无需全局搜索替换。
 *
 * Prompt 相关配置（内容较长，单独存放）：
 *   - 内置润色模式 → src/services/store.ts 的 BUILTIN_PRESETS
 *   - 应用 Prompt 规则 → src/services/personalization/defaults.ts 的 BUILTIN_APP_RULES
 */

import { DEFAULT_HANDS_FREE_SHORTCUT, DEFAULT_PTT_SHORTCUT } from '@/lib/platform'

export const DEFAULTS: Record<string, unknown> = {

  // ── 界面语言 ──
  // 只管**界面文字**，不改识别语种。设置层已经有三个"语言"了
  // （localAsr.language / server.language 是 ASR 语种，Preset 决定输出语种），
  // 所以这个键必须带 ui. 前缀，别再起名叫 language。
  'ui.language': 'auto', // 可选: 'auto'（跟随系统）| 'zh-CN' | 'en'

  // ── 工作模式 ──
  workMode: 'server', // 可选: 'server' | 'cloud_api' | 'local'

  // ── 快捷键 ──
  // 按住说话。旧单键保持 DOM code；组合键使用物理 code 格式，如 'ControlLeft+MetaLeft' 或 'ControlLeft+KeyK'。
  // ⚠️ 默认键不能用 Shift：长按右 Shift 约 8 秒会触发 Windows 筛选键，导致松开后录音停不下来
  // （详见 lib/shortcutKeys.ts 的 PTT_FORBIDDEN_CODES）。这里曾经是 'ShiftRight'，
  // 于是每台新装的机器开箱就绑在那个键上。右 Ctrl 位置相近、没有辅助功能陷阱，
  // 也不和免提的默认键（右 Alt）撞。
  // ⚠️ 改这里要同步 Rust：src-tauri/src/storage/mod.rs 的种子默认值、main.rs 的兜底、
  // keyboard/mod.rs 的 PttKeyConfig::fallback()。
  shortcutPTT: DEFAULT_PTT_SHORTCUT,
  shortcutHandsFree: DEFAULT_HANDS_FREE_SHORTCUT,

  // ── 麦克风 ──
  selectedMic: '', // 设备 ID，空字符串 = 系统默认
  muteSystemAudioWhileRecording: false, // 按住说话期间静音系统其他声音（防外放被麦克风回采）。默认关闭
  // 浏览器降噪。默认开（底噪大的机器需要）；降噪按"人听得舒服"优化，
  // 可能削掉 ASR 要的细节，麦克风环境干净时可以关掉对比准确度。
  micNoiseSuppression: true,

  // ── 文本插入 ──
  protectClipboard: true, // 插入文本后自动还原剪贴板为插入前内容，避免占用用户剪贴板。默认开启

  // ── AI 校对 ──
  aiEnabled: true, // 是否开启 AI 校对。可选: true | false
  aiPromptAppend: '', // 全局附加 prompt
  'ai.builtinPromptLanguage': 'zh-CN', // 内置 Prompt 内容语言；与界面语言相互独立

  // ── AI 供应商 ──
  // 下面这四个是**运行时生效的那一份**，录音链路、历史重跑、诊断页、反馈上报都只认它们。
  // 它们由「AI 服务」页在启用/保存时整份写入，不要在别处单独改其中一个
  // （历史 bug：切了模型但地址和密钥还是上一家的）。
  'cloudAi.provider': 'deepseek', // 可选: 'deepseek' | 'openai_compat' | 'doubao' | 'qwen' | 'mimo' | 'ollama'
  'cloudAi.apiUrl': '',
  'cloudAi.apiKey': '',
  'cloudAi.model': '',
  // 「AI 服务」列表本身：一条 = 供应商 + 地址 + 密钥 + 模型，只有设置页读写。
  'cloudAi.profiles': [],
  'cloudAi.activeProfileId': '',
  // 老的「每供应商一组配置」（cloudAi.<provider>.*）只摊平迁移一次。
  // 没有这个标记，用户删光列表后下次进页面又会被"救回来"。老键一律不删，方便降级。
  'cloudAi.profilesMigrated': false,

  // ── ASR（云 API）──
  'cloudAsr.provider': 'doubao_v2', // 可选: 'doubao_v2' | 'qwen' | 'qwen_realtime' | 'qwen_omni' | 'qwen_omni_turbo' | 'mimo'
  // 运行时读的「本次生效凭据」镜像。豆包按控制台代次算出来后写进这两个键：
  // 新版控制台只有一个 API Key、appId 必为空串（Rust 侧靠它区分两代鉴权头）。
  'cloudAsr.apiKey': '',
  'cloudAsr.appId': '', // 仅豆包旧版控制台需要
  // 豆包控制台代次与各自的密钥（两代的密钥不是同一个东西，分开存，切换不丢）
  'cloudAsr.doubao.console': 'new', // 'new' = 只要 API Key | 'legacy' = App ID + Access Token
  'cloudAsr.doubao.consoleKey': '', // 新版控制台的 API Key
  'cloudAsr.omniSystemPrompt': '', // 千问 Omni 模式的 system prompt
  // 服务列表：一张卡 = 一份完整配置（供应商 + 该平台凭据），同一家可存多份
  'cloudAsr.profiles': [],
  'cloudAsr.activeProfileId': '',
  // 已经自动补建过哪些服务。刻意不是「迁移完成」那种布尔标记 —— 记「补过谁」才能
  // 既在逻辑修好后自愈，又不把用户主动删掉的卡救回来。
  'cloudAsr.autoCreatedProviders': [],

  // ── ASR（本地）──
  // 可选值就是 catalog.rs 里那几个 id：'sensevoice-small-gguf'（默认，最快）
  // | 'funasr-nano-2512-gguf' | 'qwen3-asr-0.6b-gguf'
  // | 'qwen3-asr-1.7b-q4-gguf' | 'qwen3-asr-1.7b-gguf'（最准）
  'localAsr.modelId': 'sensevoice-small-gguf',
  'localAsr.language': 'auto', // 可选: 'auto' | 'zh' | 'en' | 'ja' | 'ko'
  // GGUF 权重目前只发在 HuggingFace（handy-computer 组织下），国内走镜像更稳。
  // 值要和 catalog 里 DownloadSource.source 的名字完全一致，否则会回落到第一个源。
  'localAsr.downloadSource': 'HuggingFace Mirror', // 可选: 'HuggingFace Mirror' | 'HuggingFace'
  'localAsr.model': '',
  // GGUF 引擎的计算后端偏好。'auto' = 有 GPU 用 GPU、没有自动用 CPU。
  // 没装 GPU 加速包的机器永远是 CPU，这个值不影响功能，只影响速度。
  'localAsr.accelerator': 'auto', // 可选: 'auto' | 'cpu' | 'gpu'
  // 本地模型空闲多少分钟后卸载（实测释放 350 MB ~ 2.6 GB 内存）。0 = 从不卸载。
  // 默认常驻，保证下一次识别无需重新付“加载 + 预热”的等待；内存紧张的用户可在
  // 本地模式设置中选择空闲 10 / 30 / 60 分钟后自动释放。
  'localAsr.unloadIdleMinutes': 0,

  // ── 服务器 ──
  // 可选: 'auto' | 'zh' | 'en'。随每次识别发给服务端，由 asr.py 的 _resolve_language
  // 映射成 Chinese / English；'auto' = 交给模型自检。
  // （注释原来写的是 'Chinese' | 'English' | 'Cantonese'，那是服务端内部的取值，
  //   客户端从来没写过这几个字符串。）
  'server.language': 'auto',

  // ── 悬浮窗 ──
  overlayWaveTheme: 'black-rainbow', // 可选: 'black-rainbow' | 'black-blue' | 'black-white'
  overlayShowDuration: true, // 是否显示录音时长。可选: true | false
  overlayWidth: 'short', // 可选: 'short' | 'medium' | 'long'

  // ── 流式实时显示 ──
  // 打开后，支持流式的 ASR 模型（豆包、千问实时）在识别阶段会把实时文字显示在悬浮窗上。
  // 识别完成后文本仍会照常交给 AI 处理。可选: true | false
  streamingDisplayEnabled: false,

  // ── 热词注入 AI 提示词 ──
  // 打开后，AI 整理时会收到你的热词表，帮助纠正/保留专有名词。默认关闭。可选: true | false
  injectHotwordsToPrompt: false,

  // ── 提示音 ──
  readySoundEnabled: true, // 录音就绪提示音。可选: true | false

  // ── 应用设置 ──
  // autoCheckUpdate 在界面上已经没有开关了（更新是必走的：后台下载 + 用户点击或退出时安装）。
  // 这里保留默认值与读取，是给更新链路自己出故障时留一条不用发新版的止血通道
  // （0.0.8 那次更新器把用户锁在死循环里，当时只能靠这个开关关掉）。
  autoCheckUpdate: true, // 是否检查更新。可选: true | false
  // 已下载待安装的更新包 { version, filePath, sha512 }。Rust 侧退出时会读它做兜底安装。
  pendingUpdate: null,
  historyEnabled: true, // 保存历史记录（文本+录音）。关闭后不再保存新记录，适合共享电脑。可选: true | false
  audioRetentionEnabled: true, // 保留录音文件。可选: true | false
  audioRetentionDays: -1, // 录音保留天数。可选: 7 | 30 | 90 | -1（永久）
  logRetentionDays: 30, // 日志保留天数。可选: 7 | 15 | 30 | 90

  // ── 文本后处理（不依赖 AI 的客户端文本规范化）──
  textPostProcess: {
    autoSegment: true, // 智能分段（仅极速模式生效），默认开启
    normalizeNumbers: true, // 数字规范化：百分之/小数/分之/结构化整数，默认开启
    stripTrailingPunctuation: false, // 去除句末标点
    punctuationToSpace: false, // 标点符号替换为空格
  },

  // ── 热词 ──
  hotwordLearning: null,

  // ── 引导 ──
  onboardingVersion: '', // 已完成引导的版本号，空字符串 = 未完成

  // ── 远程公告 ──
  dismissedNoticeIds: [], // 已被用户关闭的公告 id 列表
}

/**
 * 获取指定 key 的默认值。
 * 如果 key 不在 DEFAULTS 中，返回提供的 fallback。
 */
export function getDefault<T>(key: string, fallback?: T): T {
  if (key in DEFAULTS) {
    return DEFAULTS[key] as T
  }
  return fallback as T
}
