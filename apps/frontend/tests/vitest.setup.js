import { act } from 'react'
import { vi, expect, afterEach } from 'vitest'
import * as matchers from '@testing-library/jest-dom/matchers'
import { cleanup } from '@testing-library/react'

expect.extend(matchers)

afterEach(() => {
  cleanup()
})

vi.mock('react-dom/test-utils', () => ({
  act,
}))
