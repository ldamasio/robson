const { useState: useTIState } = React;

function TradingIntent() {
  const [stage, setStage] = useTIState(1); // 0 arm, 1 detect, 2 execute

  const steps = [
    { id: 0, label: 'Arm', status: 'done' },
    { id: 1, label: 'Detect', status: stage >= 1 ? (stage === 1 ? 'active' : 'done') : 'pending' },
    { id: 2, label: 'Execute', status: stage >= 2 ? (stage === 2 ? 'active' : 'done') : 'pending' },
  ];

  const checks = [
    { label: 'Symbol resolved', value: 'BTCUSDT', ok: true },
    { label: 'Technical stop chart-derived', value: '2nd S/R level · 15m', ok: true },
    { label: 'Stop distance within bounds', value: '0.96% · below floor 1.00%', ok: false },
    { label: 'Worst-case loss within 1% cap', value: '0.72% capital_base', ok: true },
    { label: 'Monthly budget remaining', value: '2.10% of 4.00%', ok: true },
  ];

  return (
    <div className="rbx-panel">
      <div className="rbx-panel__head">
        <div>
          <Eyebrow>Armed position · pos-2847 · 2026-04-18T09:14:22Z</Eyebrow>
          <h2 className="rbx-panel__title">LONG · BTCUSDT · confirmed_trend</h2>
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
          Approval mode: <code>human_confirmation</code>. The risk-approved entry holds until the operator confirms. A failed check re-arms the detector with backoff.
        </div>
        <div style={{display:'flex', gap:8}}>
          <Button variant="secondary">Cancel arm</Button>
          <Button variant="accent" icon="arrow-right" onClick={() => setStage(2)} disabled={checks.some(c=>!c.ok)}>
            Confirm entry
          </Button>
        </div>
      </div>
    </div>
  );
}

Object.assign(window, { TradingIntent });
