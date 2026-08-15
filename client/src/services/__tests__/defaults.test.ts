import { describe, expect, it } from 'vitest'
import { getDefault } from '../defaults'

describe('应用默认设置', () => {
  it('首次启动使用本地工作模式', () => {
    expect(getDefault('workMode')).toBe('local')
  })

  it('首次启动使用黑底白色波形主题', () => {
    expect(getDefault('overlayWaveTheme')).toBe('black-white')
  })
})
