import { act } from 'react'
import { vi } from 'vitest'

vi.mock('react-dom/test-utils', () => ({
  act,
}))
