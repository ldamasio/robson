import type { EntryPreview } from '$api/robson';

export type PreviewSeverity = 'ok' | 'warn' | 'reject';

/** What the operator must weigh before confirming the arm. */
export function previewSeverity(p: EntryPreview): PreviewSeverity {
  if (p.rejection) return 'reject';
  if (p.warnings.length > 0) return 'warn';
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
    lines.push(label('SIZE', `${z.quantity} · notional ${fmtNum(z.notional)}`));
    lines.push(
      label('RISK', `${fmtNum(z.planned_risk)} USDT · ${z.planned_risk_pct_of_capital.toFixed(2)}% of capital ${fmtNum(p.capital_base)}`),
    );
    if (z.executable_trigger != null) {
      const span = z.executable_span != null ? ` · span ${fmtNum(z.executable_span)}` : '';
      lines.push(label('TRIGGER', `${fmtNum(z.executable_trigger)}${span}`));
    }
  }
  if (p.rejection) lines.push(label('REJECTED', p.rejection));
  return lines;
}

function fmtNum(n: number): string {
  return n.toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 2 });
}
