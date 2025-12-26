import { act } from 'react'
import { vi } from 'vitest'
import '@testing-library/jest-dom'

vi.mock('react-dom/test-utils', () => ({
  act,
}))
