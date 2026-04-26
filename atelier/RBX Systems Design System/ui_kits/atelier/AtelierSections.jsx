// Atelier surfaces — the "laboratory" view of RBX products and experiments.

const AtelierHero = () => (
  <section style={{ padding: '120px 0 80px' }}>
    <Container>
      <div style={{ display: 'flex', alignItems: 'center', gap: 16, marginBottom: 28 }}>
        <StatusPill tone="info">atelier / v1</StatusPill>
        <span style={{ fontFamily: 'var(--rbx-font-mono)', fontSize: 11, color: 'var(--rbx-fg-dim)', letterSpacing: '0.04em' }}>
          LIVE · {new Date().getUTCFullYear()}
        </span>
      </div>
      <h1 style={{
        fontSize: 72, fontWeight: 300, lineHeight: 1.02, letterSpacing: '-0.02em',
        color: 'var(--rbx-fg)', margin: 0, maxWidth: '16ch',
      }}>
        Where systems<br/>are conceived,<br/>
        <span style={{ color: 'var(--rbx-accent)' }}>proven, and released.</span>
      </h1>
      <p style={{
        marginTop: 32, fontSize: 18, lineHeight: 1.55, fontWeight: 300,
        color: 'var(--rbx-fg-muted)', maxWidth: '44rem',
      }}>
        The atelier is where RBX designs and matures the infrastructure that goes into production. Public experiments, open telemetry, internal platforms and active research.
      </p>
    </Container>
  </section>
);

const ATELIER_ROWS = [
  { id: 'robson', name: 'Robson', subtitle: 'Directional intelligence engine', phase: 'Institutionalized',
    metrics: [{ k: 'uptime', v: '99.97%' }, { k: 'p50', v: '38ms' }, { k: 'agents', v: '12' }], tone: 'ok' },
  { id: 'strategos', name: 'Strategos', subtitle: 'Operating system for cognitive coordination', phase: 'Structuring',
    metrics: [{ k: 'uptime', v: '99.84%' }, { k: 'p50', v: '112ms' }, { k: 'agents', v: '6' }], tone: 'ok' },
  { id: 'truthmetal', name: 'TruthMetal', subtitle: 'Canonical truth control plane', phase: 'Seed',
    metrics: [{ k: 'uptime', v: '—' }, { k: 'p50', v: '—' }, { k: 'agents', v: '1' }], tone: 'neutral' },
  { id: 'thalamus', name: 'Thalamus', subtitle: 'Routing layer for analytical signals', phase: 'Seed',
    metrics: [{ k: 'uptime', v: '—' }, { k: 'p50', v: '—' }, { k: 'agents', v: '—' }], tone: 'neutral' },
  { id: 'argos', name: 'Argos Radar', subtitle: 'Market surveillance platform', phase: 'Seed',
    metrics: [{ k: 'uptime', v: '—' }, { k: 'p50', v: '—' }, { k: 'agents', v: '—' }], tone: 'neutral' },
  { id: 'eden', name: 'Eden', subtitle: 'Internal developer platform', phase: 'Seed',
    metrics: [{ k: 'uptime', v: '—' }, { k: 'p50', v: '—' }, { k: 'agents', v: '—' }], tone: 'neutral' },
];

const AtelierIndex = () => {
  const [hover, setHover] = useState(null);
  return (
    <section style={{ padding: '64px 0', borderTop: '1px solid var(--rbx-line)' }}>
      <Container>
        <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'baseline', marginBottom: 32 }}>
          <div style={{ display: 'flex', alignItems: 'baseline', gap: 16 }}>
            <Eyebrow>Index</Eyebrow>
            <span style={{ fontFamily: 'var(--rbx-font-mono)', fontSize: 11, color: 'var(--rbx-fg-dim)', letterSpacing: '0.04em' }}>
              {ATELIER_ROWS.length} · SORTED BY PHASE
            </span>
          </div>
          <div style={{ display: 'flex', gap: 8 }}>
            <Button variant="ghost" size="sm"><Icon name="search" size={14} /></Button>
            <Button variant="outline" size="sm">+ New experiment</Button>
          </div>
        </div>

        <div style={{ borderTop: '1px solid var(--rbx-line-strong)' }}>
          {/* Header row */}
          <div style={{
            display: 'grid', gridTemplateColumns: '32px 1.4fr 2fr 1fr 1fr 1fr 160px',
            gap: 16, padding: '12px 16px',
            borderBottom: '1px solid var(--rbx-line)',
            fontFamily: 'var(--rbx-font-mono)', fontSize: 10,
            textTransform: 'uppercase', letterSpacing: '0.14em',
            color: 'var(--rbx-fg-dim)',
          }}>
            <div>#</div><div>Name</div><div>Description</div>
            <div>Uptime</div><div>p50</div><div>Agents</div>
            <div style={{ textAlign: 'right' }}>Phase</div>
          </div>
          {ATELIER_ROWS.map((r, i) => (
            <div key={r.id}
              onMouseEnter={() => setHover(r.id)} onMouseLeave={() => setHover(null)}
              style={{
                display: 'grid', gridTemplateColumns: '32px 1.4fr 2fr 1fr 1fr 1fr 160px',
                gap: 16, padding: '20px 16px', alignItems: 'center',
                borderBottom: '1px solid var(--rbx-line)',
                background: hover === r.id ? 'var(--rbx-surface-1)' : 'transparent',
                transition: 'background 200ms var(--rbx-ease)',
                cursor: 'pointer',
              }}>
              <div style={{ fontFamily: 'var(--rbx-font-mono)', fontSize: 11, color: 'var(--rbx-fg-dim)' }}>
                {String(i + 1).padStart(2, '0')}
              </div>
              <div>
                <div style={{ fontSize: 15, fontWeight: 500, color: 'var(--rbx-fg)', letterSpacing: '-0.01em' }}>{r.name}</div>
                <div style={{ fontFamily: 'var(--rbx-font-mono)', fontSize: 10, color: 'var(--rbx-fg-dim)', letterSpacing: '0.04em', marginTop: 4 }}>
                  rbx.ia.br/{r.id}
                </div>
              </div>
              <div style={{ fontSize: 13, color: 'var(--rbx-fg-muted)', lineHeight: 1.5 }}>{r.subtitle}</div>
              {r.metrics.map((m, j) => (
                <div key={j} style={{
                  fontFamily: 'var(--rbx-font-mono)', fontSize: 13,
                  color: m.v === '—' ? 'var(--rbx-fg-faint)' : 'var(--rbx-fg)',
                  fontVariantNumeric: 'tabular-nums',
                }}>{m.v}</div>
              ))}
              <div style={{ display: 'flex', justifyContent: 'flex-end' }}>
                <PhaseTag phase={r.phase} />
              </div>
            </div>
          ))}
        </div>
      </Container>
    </section>
  );
};

const AtelierMetrics = () => {
  const cards = [
    { k: 'ACTIVE SYSTEMS', v: '6', d: 'In public atelier' },
    { k: 'INSTITUTIONALIZED', v: '1', d: 'Operational under SLO' },
    { k: 'OPEN COMMITS', v: '2,418', d: 'Across all products · 30d' },
    { k: 'UPTIME (AGG)', v: '99.92%', d: 'Public SLO surface' },
  ];
  return (
    <section style={{ padding: '64px 0', borderTop: '1px solid var(--rbx-line)' }}>
      <Container>
        <Eyebrow style={{ marginBottom: 32 }}>Public telemetry</Eyebrow>
        <div style={{ display: 'grid', gridTemplateColumns: 'repeat(4, 1fr)', gap: 0, border: '1px solid var(--rbx-line)', borderRadius: 8, overflow: 'hidden' }}>
          {cards.map((c, i) => (
            <div key={i} style={{
              padding: 28, background: 'var(--rbx-surface-1)',
              borderRight: i < 3 ? '1px solid var(--rbx-line)' : 0,
            }}>
              <div style={{
                fontFamily: 'var(--rbx-font-mono)', fontSize: 10, textTransform: 'uppercase',
                letterSpacing: '0.14em', color: 'var(--rbx-fg-dim)',
              }}>{c.k}</div>
              <div style={{
                marginTop: 16, fontSize: 44, fontWeight: 300,
                color: 'var(--rbx-fg)', letterSpacing: '-0.02em',
                fontVariantNumeric: 'tabular-nums',
              }}>{c.v}</div>
              <div style={{ marginTop: 12, fontSize: 12, color: 'var(--rbx-fg-muted)' }}>{c.d}</div>
            </div>
          ))}
        </div>
      </Container>
    </section>
  );
};

const AtelierTimeline = () => {
  const entries = [
    { t: '2026-03-14 09:12 UTC', p: 'Robson', m: 'Strategy v2.4 promoted. Tail p99 improved by 18%.', tone: 'ok' },
    { t: '2026-03-12 22:41 UTC', p: 'Strategos', m: 'New planner module wired to Thalamus bus.', tone: 'info' },
    { t: '2026-03-11 18:03 UTC', p: 'TruthMetal', m: 'Parameter registry schema v0.2 — breaking changes in mode field.', tone: 'warn' },
    { t: '2026-03-10 14:22 UTC', p: 'Argos', m: 'Surveillance coverage extended to 4 new symbols.', tone: 'info' },
    { t: '2026-03-09 11:50 UTC', p: 'Eden', m: 'Cluster maintenance window completed. No downstream impact.', tone: 'neutral' },
  ];
  return (
    <section style={{ padding: '64px 0', borderTop: '1px solid var(--rbx-line)' }}>
      <Container>
        <Eyebrow style={{ marginBottom: 32 }}>Recent activity</Eyebrow>
        <div style={{ display: 'flex', flexDirection: 'column' }}>
          {entries.map((e, i) => (
            <div key={i} style={{
              display: 'grid', gridTemplateColumns: '220px 140px 1fr auto',
              gap: 24, padding: '18px 0', alignItems: 'center',
              borderTop: i === 0 ? '1px solid var(--rbx-line)' : 0,
              borderBottom: '1px solid var(--rbx-line)',
            }}>
              <div style={{ fontFamily: 'var(--rbx-font-mono)', fontSize: 11, color: 'var(--rbx-fg-dim)' }}>{e.t}</div>
              <div style={{ fontSize: 13, fontWeight: 500, color: 'var(--rbx-fg)' }}>{e.p}</div>
              <div style={{ fontSize: 13, color: 'var(--rbx-fg-muted)', lineHeight: 1.5 }}>{e.m}</div>
              <StatusPill tone={e.tone}>{e.tone}</StatusPill>
            </div>
          ))}
        </div>
      </Container>
    </section>
  );
};

Object.assign(window, { AtelierHero, AtelierIndex, AtelierMetrics, AtelierTimeline });
