const { useState: useTIState } = React;

function TradingIntent() {
  const [stage, setStage] = useTIState(1); // 0 plan, 1 validate, 2 execute

  const steps = [
    { id: 0, label: 'Plan', status: 'done' },
    { id: 1, label: 'Validate', status: stage >= 1 ? (stage === 1 ? 'active' : 'done') : 'pending' },
    { id: 2, label: 'Execute', status: stage >= 2 ? (stage === 2 ? 'active' : 'done') : 'pending' },
  ];

  const checks = [
    { label: 'Symbol resolved', value: 'BTCUSDT', ok: true },
    { label: 'Quantity within sizing policy', value: '0.00482 · 0.72% equity', ok: true },
    { label: 'Stop distance', value: '0.96% · below floor 1.00%', ok: false },
    { label: 'Liquidation headroom', value: '12.40%', ok: true },
    { label: 'Max concurrent positions', value: '2 / 5', ok: true },
  ];

  return (
    <div className="rbx-panel">
      <div className="rbx-panel__head">
        <div>
          <Eyebrow>Plan · plan-2847 · 2026-04-18T09:14:22Z</Eyebrow>
          <h2 className="rbx-panel__title">BUY · BTCUSDT · 0.00482 @ 67,420.50</h2>
        </div>
        <div style={{display:'flex', gap:8}}>
          <Button variant="secondary" icon="copy">Duplicate</Button>
          <Button variant="ghost" icon="x">Discard</Button>
        </div>
      </div>

      <div className="rbx-pipeline">
        {steps.map((s, i) => (
          <React.Fragment key={s.id}>
            <div className={`rbx-stage is-${s.status}`}>
              <div className="rbx-stage__num">{String(i+1).padStart(2,'0')}</div>
              <div className="rbx-stage__label">{s.label}</div>
            </div>
            {i < steps.length - 1 && <div className={`rbx-stage__sep ${s.status === 'done' ? 'is-done' : ''}`} />}
          </React.Fragment>
        ))}
      </div>

      <div className="rbx-panel__body">
        <Eyebrow>Validation checks · {checks.filter(c=>c.ok).length}/{checks.length} passed</Eyebrow>
        <div className="rbx-checklist">
          {checks.map((c, i) => (
            <div key={i} className={`rbx-check ${c.ok ? 'is-ok' : 'is-fail'}`}>
              <i data-lucide={c.ok ? 'check' : 'alert-triangle'}></i>
              <div className="rbx-check__label">{c.label}</div>
              <div className="rbx-check__value">{c.value}</div>
            </div>
          ))}
        </div>
      </div>

      <div className="rbx-panel__foot">
        <div className="rbx-foot-note">
          Execution requires <code>--live --acknowledge-risk</code>. One validation failure must clear before stage advance.
        </div>
        <div style={{display:'flex', gap:8}}>
          <Button variant="secondary">Run dry-run</Button>
          <Button variant="accent" icon="arrow-right" onClick={() => setStage(2)} disabled={checks.some(c=>!c.ok)}>
            Acknowledge risk
          </Button>
        </div>
      </div>
    </div>
  );
}

Object.assign(window, { TradingIntent });
