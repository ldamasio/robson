import type { EntryPreview } from '$api/robson';

export type PreviewSeverity = 'ok' | 'warn' | 'reject';

/**
 * What the operator must weigh before confirming the arm. Only an explicit
 * domain rejection blocks; an unavailable preview (no candles, metadata down)
 * is a reason for caution, never a verdict on the arm.
 */
export function previewSeverity(p: EntryPreview): PreviewSeverity {
  if (p.rejection) return 'reject';
  if (p.warnings.length > 0 || p.unavailable) return 'warn';
  return 'ok';
}

/** Monospace lines for the preview panel, same shape as the position card. */
export function previewLines(p: EntryPreview): string[] {
  const label = (tag: string, detail: string) => `${tag.padEnd(10)}${detail}`;
  const lines: string[] = [];
  if (p.reference_price > 0) lines.push(label('REFERENCE', fmtNum(p.reference_price)));
  const a = p.analysis;
  if (a) {
    lines.push(
      label('STOP', `${fmtNum(a.technical_stop)} · ${fmtNum(a.stop_distance)} pts · ${a.stop_distance_pct.toFixed(2)}%`),
    );
    const method =
      a.method === 'atr_fallback'
        ? 'ATR fallback, no chart level in bounds'
        : a.method === 'swing_point'
          ? `swing level ${a.level_n ?? '?'}`
          : a.method;
    lines.push(label('METHOD', `${method} · ${a.confidence.toUpperCase()} confidence`));
    if (a.invalidation_guard_level != null) {
      lines.push(label('GUARD', fmtNum(a.invalidation_guard_level)));
    }
  }
  const z = p.sizing;
  if (z) {
    lines.push(label('SIZE', `${z.quantity} · notional ${fmtMoney(z.notional)}`));
    lines.push(
      label('RISK', `${fmtMoney(z.planned_risk)} USDT · ${z.planned_risk_pct_of_capital.toFixed(2)}% of capital ${fmtMoney(p.capital_base)}`),
    );
    if (z.executable_trigger != null) {
      const span = z.executable_span != null ? ` · span ${fmtNum(z.executable_span)}` : '';
      lines.push(label('TRIGGER', `${fmtNum(z.executable_trigger)}${span}`));
    }
  }
  if (p.unavailable) lines.push(label('NO DATA', p.unavailable));
  if (p.rejection) lines.push(label('REJECTED', p.rejection));
  return lines;
}

/** Monetary totals (USDT): always two decimals. */
function fmtMoney(n: number): string {
  return n.toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 2 });
}

/**
 * Prices and spans with at least five significant digits and never fewer
 * than two decimals, so BTC prints as 83,454.50 while a 0.1234 reference and
 * a 0.1224 stop never both print as 0.12.
 */
export function fmtNum(n: number): string {
  const abs = Math.abs(n);
  const decimals =
    abs === 0 ? 2 : Math.min(8, Math.max(2, 4 - Math.floor(Math.log10(abs))));
  return n.toLocaleString('en-US', { minimumFractionDigits: decimals, maximumFractionDigits: decimals });
}
