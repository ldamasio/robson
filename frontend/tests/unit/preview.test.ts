import { describe, it, expect } from 'vitest';
import { previewLines, previewSeverity } from '$lib/presentation/preview';
import { normalizeEntryPreview, type EntryPreview } from '$api/robson';

function preview(overrides: Partial<EntryPreview> = {}): EntryPreview {
  return {
    symbol: 'BTCUSDT',
    side: 'Short',
    reference_price: 83454.5,
    capital_base: 1710.81,
    analysis: {
      technical_stop: 84134.2,
      stop_distance: 679.7,
      stop_distance_pct: 0.81,
      method: 'swing_point',
      level_n: 2,
      confidence: 'high',
      invalidation_guard_level: null
    },
    sizing: {
      quantity: 0.018,
      notional: 1502.18,
      planned_risk: 16.81,
      planned_risk_pct_of_capital: 0.98,
      executable_trigger: 84218.4,
      executable_span: 763.9
    },
    rejection: null,
    warnings: [],
    ...overrides
  };
}

describe('previewSeverity', () => {
  it('is ok with a chart stop and no warnings', () => {
    expect(previewSeverity(preview())).toBe('ok');
  });
  it('warns on advisory notes', () => {
    expect(previewSeverity(preview({ warnings: ['ATR fallback'] }))).toBe('warn');
  });
  it('rejects when the arm would be rejected', () => {
    expect(previewSeverity(preview({ rejection: 'stop too tight', warnings: ['x'] }))).toBe('reject');
  });
});

describe('previewLines', () => {
  it('renders the live 2026-10-13 short preview', () => {
    const lines = previewLines(preview());
    expect(lines[0]).toMatch(/^REFERENCE\s+83,454.50$/);
    expect(lines[1]).toBe('STOP      84,134.20 · 679.70 pts · 0.81%');
    expect(lines[2]).toBe('METHOD    swing level 2 · HIGH confidence');
    expect(lines[3]).toBe('SIZE      0.018 · notional 1,502.18');
    expect(lines[4]).toBe('RISK      16.81 USDT · 0.98% of capital 1,710.81');
    expect(lines[5]).toBe('TRIGGER   84,218.40 · span 763.90');
  });

  it('names the fallback and shows a rejection line', () => {
    const lines = previewLines(
      preview({
        analysis: { ...preview().analysis!, method: 'atr_fallback', level_n: null, confidence: 'low' },
        sizing: null,
        rejection: 'Stop too tight for the minimum distance'
      })
    );
    expect(lines.some((l) => l.includes('ATR fallback, no chart level in bounds · LOW confidence'))).toBe(true);
    expect(lines[lines.length - 1]).toMatch(/^REJECTED\s+Stop too tight/);
  });
});

describe('normalizeEntryPreview', () => {
  it('parses decimal strings and tolerates missing blocks', () => {
    const p = normalizeEntryPreview({
      symbol: 'BTCUSDT',
      side: 'Long',
      reference_price: '100.5',
      capital_base: '1000',
      analysis: null,
      sizing: null,
      rejection: 'no candles',
      warnings: []
    });
    expect(p.reference_price).toBe(100.5);
    expect(p.analysis).toBeNull();
    expect(p.rejection).toBe('no candles');
  });
});
