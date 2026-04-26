// Robson control plane — a dense, operational product surface.
// Shows how RBX primitives compose into a real product UI.

const ProductShell = ({ children }) => (
  <div style={{
    display: 'grid', gridTemplateColumns: '232px 1fr',
    minHeight: '100vh',
  }}>
    <ProductSidebar />
    <div style={{ background: 'var(--rbx-ink)', minWidth: 0 }}>{children}</div>
  </div>
);

const ProductSidebar = () => {
  const nav = [
    { g: 'Operations', items: [
      { i: 'activity', l: 'Overview', active: true },
      { i: 'server', l: 'Strategies' },
      { i: 'git', l: 'Executions' },
      { i: 'globe', l: 'Markets' },
    ]},
    { g: 'Governance', items: [
      { i: 'check', l: 'Parameters' },
      { i: 'check', l: 'Audit log' },
      { i: 'check', l: 'Policies' },
    ]},
    { g: 'System', items: [
      { i: 'check', l: 'Integrations' },
      { i: 'check', l: 'Team' },
    ]},
  ];
  return (
    <aside style={{
      background: 'var(--rbx-surface-3)',
      borderRight: '1px solid var(--rbx-line)',
      padding: '20px 0',
      display: 'flex', flexDirection: 'column',
      position: 'sticky', top: 0, height: '100vh',
    }}>
      <div style={{ padding: '4px 20px 24px' }}>
        <div style={{ display: 'flex', alignItems: 'center', gap: 12 }}>
          <RbxMark size={28} />
          <div style={{ display: 'flex', flexDirection: 'column', lineHeight: 1 }}>
            <span style={{ fontSize: 13, fontWeight: 600, color: 'var(--rbx-fg)', letterSpacing: '-0.01em' }}>Robson</span>
            <span style={{ fontFamily: 'var(--rbx-font-mono)', fontSize: 9, color: 'var(--rbx-fg-dim)', letterSpacing: '0.14em', textTransform: 'uppercase', marginTop: 3 }}>RBX / Control Plane</span>
          </div>
        </div>
      </div>

      <div style={{ padding: '0 12px 16px' }}>
        <div style={{
          display: 'flex', alignItems: 'center', gap: 8,
          padding: '8px 10px', border: '1px solid var(--rbx-line)',
          borderRadius: 6, background: 'var(--rbx-surface-2)',
          color: 'var(--rbx-fg-dim)',
        }}>
          <Icon name="search" size={13} />
          <span style={{ fontSize: 12 }}>Quick jump</span>
          <span style={{ marginLeft: 'auto', fontFamily: 'var(--rbx-font-mono)', fontSize: 10, color: 'var(--rbx-fg-faint)', border: '1px solid var(--rbx-line)', padding: '1px 5px', borderRadius: 3 }}>⌘K</span>
        </div>
      </div>

      {nav.map((sec, i) => (
        <div key={i} style={{ padding: '8px 12px' }}>
          <div style={{
            padding: '6px 10px', fontFamily: 'var(--rbx-font-mono)', fontSize: 10,
            textTransform: 'uppercase', letterSpacing: '0.14em', color: 'var(--rbx-fg-dim)',
          }}>{sec.g}</div>
          {sec.items.map((it, j) => (
            <div key={j} style={{
              display: 'flex', alignItems: 'center', gap: 10,
              padding: '7px 10px', fontSize: 13,
              color: it.active ? 'var(--rbx-fg)' : 'var(--rbx-fg-muted)',
              background: it.active ? 'var(--rbx-surface-2)' : 'transparent',
              borderRadius: 5, cursor: 'pointer',
              borderLeft: it.active ? '2px solid var(--rbx-accent)' : '2px solid transparent',
              paddingLeft: it.active ? 8 : 10,
            }}>
              <Icon name={it.i} size={14} />
              {it.l}
            </div>
          ))}
        </div>
      ))}

      <div style={{ marginTop: 'auto', padding: 16, borderTop: '1px solid var(--rbx-line)' }}>
        <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
          <div style={{
            width: 28, height: 28, borderRadius: '50%',
            background: 'var(--rbx-surface-2)', border: '1px solid var(--rbx-line)',
            display: 'flex', alignItems: 'center', justifyContent: 'center',
            fontFamily: 'var(--rbx-font-mono)', fontSize: 10, color: 'var(--rbx-fg-muted)',
          }}>LD</div>
          <div style={{ display: 'flex', flexDirection: 'column', lineHeight: 1.2, flex: 1, minWidth: 0 }}>
            <span style={{ fontSize: 12, color: 'var(--rbx-fg)', fontWeight: 500 }}>Leandro D.</span>
            <span style={{ fontFamily: 'var(--rbx-font-mono)', fontSize: 10, color: 'var(--rbx-fg-dim)' }}>operator</span>
          </div>
          <Icon name="chevron" size={14} color="var(--rbx-fg-dim)" />
        </div>
      </div>
    </aside>
  );
};

const ProductTopbar = () => (
  <header style={{
    display: 'flex', alignItems: 'center', justifyContent: 'space-between',
    padding: '14px 32px', borderBottom: '1px solid var(--rbx-line)',
    background: 'var(--rbx-ink)', position: 'sticky', top: 0, zIndex: 10,
  }}>
    <div style={{ display: 'flex', alignItems: 'center', gap: 12 }}>
      <span style={{ fontFamily: 'var(--rbx-font-mono)', fontSize: 11, color: 'var(--rbx-fg-dim)', letterSpacing: '0.04em' }}>
        PROD / SA-EAST-1
      </span>
      <span style={{ color: 'var(--rbx-line-strong)' }}>/</span>
      <span style={{ fontSize: 14, color: 'var(--rbx-fg)', fontWeight: 500 }}>Overview</span>
    </div>
    <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
      <StatusPill tone="ok">systems nominal</StatusPill>
      <Button variant="outline" size="sm"><Icon name="git" size={13} /> main · 4f8a21</Button>
      <Button variant="primary" size="sm">Deploy</Button>
    </div>
  </header>
);

const KPIRow = () => {
  const cards = [
    { k: 'Portfolio Δ (24h)', v: '+2.41%', sub: '▲ +0.18% vs session', tone: 'ok' },
    { k: 'Active strategies', v: '12', sub: '9 live · 3 paused', tone: 'neutral' },
    { k: 'Executions (24h)', v: '4,182', sub: 'p50 38ms · p99 214ms', tone: 'neutral' },
    { k: 'Risk budget', v: '62%', sub: 'threshold 85%', tone: 'warn' },
  ];
  return (
    <div style={{
      display: 'grid', gridTemplateColumns: 'repeat(4, 1fr)', gap: 0,
      border: '1px solid var(--rbx-line)', borderRadius: 8, overflow: 'hidden',
    }}>
      {cards.map((c, i) => (
        <div key={i} style={{
          padding: 24, background: 'var(--rbx-surface-1)',
          borderRight: i < 3 ? '1px solid var(--rbx-line)' : 0,
        }}>
          <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'flex-start' }}>
            <div style={{
              fontFamily: 'var(--rbx-font-mono)', fontSize: 10, textTransform: 'uppercase',
              letterSpacing: '0.14em', color: 'var(--rbx-fg-dim)',
            }}>{c.k}</div>
            <StatusDot color={c.tone === 'ok' ? 'var(--rbx-ok)' : c.tone === 'warn' ? 'var(--rbx-warn)' : 'var(--rbx-fg-muted)'} />
          </div>
          <div style={{
            marginTop: 18, fontSize: 36, fontWeight: 300,
            color: 'var(--rbx-fg)', letterSpacing: '-0.02em',
            fontVariantNumeric: 'tabular-nums',
          }}>{c.v}</div>
          <div style={{ marginTop: 8, fontFamily: 'var(--rbx-font-mono)', fontSize: 11, color: 'var(--rbx-fg-muted)' }}>{c.sub}</div>
        </div>
      ))}
    </div>
  );
};

// Simple SVG sparkline chart
const Sparkline = ({ points, color = 'var(--rbx-accent)', height = 180 }) => {
  const w = 800, h = height, pad = 20;
  const min = Math.min(...points), max = Math.max(...points);
  const span = max - min || 1;
  const scaled = points.map((v, i) => [
    pad + (i / (points.length - 1)) * (w - pad * 2),
    pad + (1 - (v - min) / span) * (h - pad * 2),
  ]);
  const path = scaled.map(([x, y], i) => `${i === 0 ? 'M' : 'L'}${x.toFixed(1)},${y.toFixed(1)}`).join(' ');
  const area = path + ` L${scaled[scaled.length-1][0]},${h-pad} L${scaled[0][0]},${h-pad} Z`;
  return (
    <svg viewBox={`0 0 ${w} ${h}`} width="100%" height={h} preserveAspectRatio="none">
      <defs>
        <linearGradient id="sparkFill" x1="0" x2="0" y1="0" y2="1">
          <stop offset="0%" stopColor={color} stopOpacity="0.16" />
          <stop offset="100%" stopColor={color} stopOpacity="0" />
        </linearGradient>
      </defs>
      {[0, 0.25, 0.5, 0.75, 1].map(t => (
        <line key={t} x1={pad} x2={w-pad} y1={pad + t*(h-pad*2)} y2={pad + t*(h-pad*2)}
          stroke="var(--rbx-line)" strokeWidth="1" strokeDasharray={t === 0 || t === 1 ? '0' : '2,4'} />
      ))}
      <path d={area} fill="url(#sparkFill)" />
      <path d={path} fill="none" stroke={color} strokeWidth="1.5" strokeLinejoin="round" strokeLinecap="round" />
    </svg>
  );
};

const PortfolioChart = () => {
  // fake plausible-looking timeseries
  const pts = [];
  let v = 100;
  for (let i = 0; i < 96; i++) { v += (Math.sin(i/7) + Math.cos(i/3.3)*0.6) * 0.8 + (i/96)*0.9; pts.push(v); }
  return (
    <Card padding={0} style={{ overflow: 'hidden' }}>
      <div style={{ padding: '20px 24px', borderBottom: '1px solid var(--rbx-line)', display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
        <div>
          <div style={{ fontSize: 14, fontWeight: 500, color: 'var(--rbx-fg)' }}>Portfolio value · 24h</div>
          <div style={{ fontFamily: 'var(--rbx-font-mono)', fontSize: 11, color: 'var(--rbx-fg-dim)', marginTop: 4, letterSpacing: '0.04em' }}>
            BRL · sa-east-1 · tick 15m
          </div>
        </div>
        <div style={{ display: 'flex', gap: 4 }}>
          {['1H', '24H', '7D', '30D', 'MAX'].map((r, i) => (
            <button key={r} style={{
              background: i === 1 ? 'var(--rbx-surface-2)' : 'transparent',
              border: '1px solid var(--rbx-line)',
              color: i === 1 ? 'var(--rbx-fg)' : 'var(--rbx-fg-dim)',
              fontFamily: 'var(--rbx-font-mono)', fontSize: 10,
              letterSpacing: '0.04em', padding: '5px 10px',
              borderRadius: 4, cursor: 'pointer',
            }}>{r}</button>
          ))}
        </div>
      </div>
      <div style={{ padding: '16px 8px 8px' }}>
        <Sparkline points={pts} />
      </div>
    </Card>
  );
};

const StrategyRow = ({ rank, name, kind, pnl, tone, state }) => (
  <div style={{
    display: 'grid', gridTemplateColumns: '32px 1.6fr 1fr 1fr 110px 36px',
    gap: 12, padding: '14px 16px', alignItems: 'center',
    borderBottom: '1px solid var(--rbx-line)',
    fontSize: 13,
  }}>
    <div style={{ fontFamily: 'var(--rbx-font-mono)', fontSize: 11, color: 'var(--rbx-fg-dim)' }}>
      {String(rank).padStart(2, '0')}
    </div>
    <div>
      <div style={{ color: 'var(--rbx-fg)', fontWeight: 500, letterSpacing: '-0.01em' }}>{name}</div>
      <div style={{ fontFamily: 'var(--rbx-font-mono)', fontSize: 10, color: 'var(--rbx-fg-dim)', marginTop: 3, letterSpacing: '0.04em' }}>{kind}</div>
    </div>
    <div style={{
      fontFamily: 'var(--rbx-font-mono)', fontSize: 13, fontVariantNumeric: 'tabular-nums',
      color: tone === 'ok' ? 'var(--rbx-ok)' : tone === 'err' ? 'var(--rbx-err)' : 'var(--rbx-fg)',
    }}>{pnl}</div>
    <div style={{ fontFamily: 'var(--rbx-font-mono)', fontSize: 11, color: 'var(--rbx-fg-muted)', letterSpacing: '0.04em' }}>
      p50 {Math.floor(20 + Math.random()*60)}ms
    </div>
    <StatusPill tone={state === 'LIVE' ? 'ok' : state === 'PAUSED' ? 'warn' : 'neutral'}>{state}</StatusPill>
    <button style={{
      background: 'transparent', border: '1px solid var(--rbx-line)',
      color: 'var(--rbx-fg-dim)', padding: '4px 6px', borderRadius: 4, cursor: 'pointer',
    }}>···</button>
  </div>
);

const StrategiesCard = () => (
  <Card padding={0}>
    <div style={{ padding: '18px 24px', borderBottom: '1px solid var(--rbx-line)', display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
      <div style={{ fontSize: 14, fontWeight: 500, color: 'var(--rbx-fg)' }}>Active strategies</div>
      <Button variant="ghost" size="sm">View all <Icon name="arrow" size={13} /></Button>
    </div>
    <StrategyRow rank={1} name="Delta-Neutral Pairs / WIN-DOL" kind="statistical arbitrage" pnl="+R$ 18,402" tone="ok" state="LIVE" />
    <StrategyRow rank={2} name="Mean Reversion / IBOV" kind="mean reversion" pnl="+R$ 9,118" tone="ok" state="LIVE" />
    <StrategyRow rank={3} name="Momentum Break / PETR4" kind="momentum" pnl="−R$ 2,340" tone="err" state="LIVE" />
    <StrategyRow rank={4} name="Overnight / VALE3" kind="carry" pnl="+R$ 4,802" tone="ok" state="PAUSED" />
    <StrategyRow rank={5} name="Vol Skew / Options" kind="derivatives" pnl="+R$ 12,004" tone="ok" state="LIVE" />
  </Card>
);

const SystemHealth = () => {
  const services = [
    { n: 'robson-executor', s: 'healthy', u: '99.99%', p: '24ms' },
    { n: 'thalamus-router', s: 'healthy', u: '99.98%', p: '12ms' },
    { n: 'truthmetal-registry', s: 'degraded', u: '99.81%', p: '188ms' },
    { n: 'argos-surveillance', s: 'healthy', u: '99.97%', p: '41ms' },
    { n: 'strategos-planner', s: 'healthy', u: '99.95%', p: '62ms' },
  ];
  return (
    <Card padding={0}>
      <div style={{ padding: '18px 24px', borderBottom: '1px solid var(--rbx-line)', display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
        <div style={{ fontSize: 14, fontWeight: 500, color: 'var(--rbx-fg)' }}>System health</div>
        <span style={{ fontFamily: 'var(--rbx-font-mono)', fontSize: 10, color: 'var(--rbx-fg-dim)', letterSpacing: '0.04em' }}>LIVE</span>
      </div>
      {services.map((s, i) => (
        <div key={i} style={{
          display: 'grid', gridTemplateColumns: '14px 1fr 70px 70px',
          gap: 12, padding: '12px 24px', alignItems: 'center',
          borderBottom: i === services.length - 1 ? 0 : '1px solid var(--rbx-line)',
          fontSize: 12,
        }}>
          <StatusDot color={s.s === 'healthy' ? 'var(--rbx-ok)' : 'var(--rbx-warn)'} />
          <div style={{ fontFamily: 'var(--rbx-font-mono)', fontSize: 12, color: 'var(--rbx-fg)', letterSpacing: '0.02em' }}>{s.n}</div>
          <div style={{ fontFamily: 'var(--rbx-font-mono)', fontSize: 11, color: 'var(--rbx-fg-muted)', fontVariantNumeric: 'tabular-nums' }}>{s.u}</div>
          <div style={{ fontFamily: 'var(--rbx-font-mono)', fontSize: 11, color: 'var(--rbx-fg-muted)', fontVariantNumeric: 'tabular-nums' }}>{s.p}</div>
        </div>
      ))}
    </Card>
  );
};

const ProductDashboard = () => (
  <ProductShell>
    <ProductTopbar />
    <div style={{ padding: 32, display: 'flex', flexDirection: 'column', gap: 24 }}>
      <div>
        <div style={{ display: 'flex', alignItems: 'baseline', justifyContent: 'space-between', marginBottom: 6 }}>
          <h1 style={{ fontSize: 28, fontWeight: 400, color: 'var(--rbx-fg)', letterSpacing: '-0.015em', margin: 0 }}>Overview</h1>
          <span style={{ fontFamily: 'var(--rbx-font-mono)', fontSize: 11, color: 'var(--rbx-fg-dim)', letterSpacing: '0.04em' }}>
            TUE · 14 MAR 2026 · 14:32 UTC
          </span>
        </div>
        <p style={{ fontSize: 14, color: 'var(--rbx-fg-muted)', margin: 0 }}>Portfolio telemetry, active strategies and system health.</p>
      </div>

      <KPIRow />
      <PortfolioChart />

      <div style={{ display: 'grid', gridTemplateColumns: '1.6fr 1fr', gap: 24 }}>
        <StrategiesCard />
        <SystemHealth />
      </div>
    </div>
  </ProductShell>
);

Object.assign(window, { ProductDashboard });
