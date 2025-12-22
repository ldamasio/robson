// @vitest-environment jsdom
import React from 'react'
import { render, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import ErrorBoundary from '../src/components/common/ErrorBoundary'

const Bomb = () => {
  throw new Error('boom')
}

describe('ErrorBoundary', () => {
  it('renders fallback when child throws', () => {
    const consoleSpy = vi.spyOn(console, 'error').mockImplementation(() => {})

    render(
      <ErrorBoundary fallback={<div>Fallback content</div>} onError={() => {}}>
        <Bomb />
      </ErrorBoundary>
    )

    expect(screen.getByText('Fallback content')).toBeTruthy()
    consoleSpy.mockRestore()
  })
})
