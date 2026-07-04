// WorkDetail.jsx — Project detail page

function WorkDetailPage({ work, onNav, theme }) {
  const isLight = theme === 'light';
  const fg  = isLight ? '#0A0A0B' : '#F5F5F3';
  const fg2 = isLight ? '#3A3B3E' : '#B4B5B8';
  const fg3 = isLight ? '#6A6B6E' : '#72747A';
  const bg  = isLight ? '#F5F5F3' : '#0A0A0B';
  const hr  = isLight ? '#C8C8C4' : '#26272C';

  const w = work || {
    client: "Enforce / BTG Pactual Group",
    role: "AI Engineer",
    years: "2025–present",
    status: "active",
    statusLabel: "Active",
    tech: "Python · FastAPI · k3s · ArgoCD · pgvector · RAG",
    line: "Production-grade AI systems for financial and legal domains.",
  };

  const metrics = [
    { label: "Time to test AI workflows", value: "days → hours" },
    { label: "Uptime target", value: "99.97%" },
    { label: "Vector search latency p99", value: "48ms" },
    { label: "Systems governed", value: "4 internal" },
  ];

  return (
    <div style={{ background: bg, minHeight: '100vh', fontFamily: "'Geist',sans-serif" }}>
      <Nav page="work" onNav={onNav} theme={theme} onToggleTheme={() => {}} />

      <div style={{ maxWidth: 1152, margin: '0 auto', padding: '100px 32px 80px' }}>

        {/* Back */}
        <button onClick={() => onNav('home')} style={{ fontFamily: "'GeistMono',monospace", fontSize: 10, letterSpacing: '0.08em', color: fg3, background: 'none', border: 'none', cursor: 'pointer', padding: 0, marginBottom: 40 }}>
          ← BACK
        </button>

        {/* Header */}
        <div style={{ borderTop: `1px solid ${hr}`, paddingTop: 16, marginBottom: 40 }}>
          <div style={{ fontFamily: "'GeistMono',monospace", fontSize: 10, fontWeight: 500, letterSpacing: '0.14em', textTransform: 'uppercase', color: fg3, marginBottom: 8 }}>
            {w.client}
          </div>
          <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'flex-start', gap: 16, flexWrap: 'wrap' }}>
            <h1 style={{ fontFamily: "'Geist',sans-serif", fontSize: 'clamp(28px, 4vw, 48px)', fontWeight: 300, color: fg, lineHeight: 1.1, letterSpacing: '-0.02em' }}>
              {w.role}
            </h1>
            <StatusPill status={w.status} label={w.statusLabel} />
          </div>
          <div style={{ fontFamily: "'GeistMono',monospace", fontSize: 11, color: fg3, marginTop: 6 }}>{w.years}</div>
        </div>

        <div style={{ display: 'grid', gridTemplateColumns: '1fr 280px', gap: 48, alignItems: 'start' }}>

          {/* Prose */}
          <div style={{ maxWidth: 672 }}>
            <p style={{ fontFamily: "'Geist',sans-serif", fontSize: 17, color: fg2, lineHeight: 1.7, marginBottom: 24 }}>
              {w.line} Work at the intersection of AI engineering, infrastructure, and compliance-aware system design.
            </p>
            <p style={{ fontFamily: "'Geist',sans-serif", fontSize: 15, color: fg2, lineHeight: 1.7, marginBottom: 24 }}>
              Designed and implemented internal LLM governance platforms, RAG pipelines, and agentic runtimes for regulated financial environments. All systems built with auditability, observability, and production reliability as first-class constraints.
            </p>
            <p style={{ fontFamily: "'Geist',sans-serif", fontSize: 15, color: fg2, lineHeight: 1.7, marginBottom: 32 }}>
              Structured Kubernetes environments (k3s + ArgoCD + GitOps) for AI service deployment. Promoted AI engineering best practices including versioning, evaluation, and compliance-aware design across the organization.
            </p>

            <div style={{ borderTop: `1px solid ${hr}`, paddingTop: 16, marginBottom: 24 }}>
              <div style={{ fontFamily: "'GeistMono',monospace", fontSize: 10, fontWeight: 500, letterSpacing: '0.14em', textTransform: 'uppercase', color: fg3, marginBottom: 10 }}>Stack</div>
              <div style={{ fontFamily: "'GeistMono',monospace", fontSize: 12, color: fg2, lineHeight: 2 }}>{w.tech}</div>
            </div>
          </div>

          {/* Metrics sidebar */}
          <div>
            <div style={{ borderTop: `1px solid ${hr}`, paddingTop: 16, marginBottom: 16 }}>
              <div style={{ fontFamily: "'GeistMono',monospace", fontSize: 10, fontWeight: 500, letterSpacing: '0.14em', textTransform: 'uppercase', color: fg3, marginBottom: 12 }}>Metrics</div>
              {metrics.map((m, i) => (
                <div key={i} style={{ marginBottom: 16 }}>
                  <div style={{ fontFamily: "'Geist',sans-serif", fontSize: 10, color: fg3, marginBottom: 3 }}>{m.label}</div>
                  <div style={{ fontFamily: "'GeistMono',monospace", fontSize: 16, color: '#D9CBA3', fontWeight: 400 }}>{m.value}</div>
                </div>
              ))}
            </div>

            {/* Screenshot placeholder */}
            <div style={{ marginTop: 24, background: isLight ? '#E8E8E6' : '#15161A', border: `1px solid ${hr}`, borderRadius: 8, height: 160, display: 'flex', alignItems: 'center', justifyContent: 'center' }}>
              <div style={{ fontFamily: "'GeistMono',monospace", fontSize: 9, color: fg3, letterSpacing: '0.08em', textTransform: 'uppercase' }}>Screenshot</div>
            </div>
          </div>
        </div>
      </div>

      {/* Footer */}
      <footer style={{ background: isLight ? '#DDDDD9' : '#1D1D22', borderTop: `1px solid ${hr}`, padding: '16px 32px', marginTop: 40 }}>
        <div style={{ maxWidth: 1152, margin: '0 auto' }}>
          <span style={{ fontFamily: "'GeistMono',monospace", fontSize: 9, letterSpacing: '0.12em', textTransform: 'uppercase', color: fg3 }}>RBX Systems · CHE-xxx.xxx.xxx</span>
        </div>
      </footer>
    </div>
  );
}

Object.assign(window, { WorkDetailPage });
