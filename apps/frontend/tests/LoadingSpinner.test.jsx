// @vitest-environment jsdom
import React from 'react'
import { render, screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import LoadingSpinner from '../src/components/common/LoadingSpinner'

describe('LoadingSpinner', () => {
  it('renders the loading label', () => {
    render(<LoadingSpinner label="Loading data..." />)
    expect(screen.getByText('Loading data...')).toBeTruthy()
  })
})
