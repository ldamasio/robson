// Marketing surfaces: Hero, Positioning, Products, Team.

const Hero = () => (
  <section style={{ padding: '120px 0 96px' }}>
    <Container>
      <div style={{ display: 'grid', gridTemplateColumns: '1.1fr 1fr', gap: 64, alignItems: 'center' }}>
        <div>
          <Eyebrow style={{ marginBottom: 24 }}>RBX Systems · Zurich · São Paulo</Eyebrow>
          <h1 style={{
            fontSize: 64, fontWeight: 300, lineHeight: 1.05, letterSpacing: '-0.015em',
            color: 'var(--rbx-fg)', margin: 0,
          }}>
            Systems engineering<br />
            <span style={{ color: 'var(--rbx-fg-muted)' }}>for operations that<br />demand control.</span>
          </h1>
          <p style={{
            marginTop: 28, fontSize: 17, lineHeight: 1.6, fontWeight: 300,
            color: 'var(--rbx-fg-muted)', maxWidth: '38rem',
          }}>
            We design platforms, automations and infrastructure for companies operating with high demands. Backend, cloud, intelligent agents and integrations built for reliability and predictable scale.
          </p>
          <div style={{ marginTop: 36, display: 'flex', gap: 12 }}>
            <Button variant="primary">Our services</Button>
            <Button variant="outline">Products</Button>
          </div>
        </div>
        <div style={{
          aspectRatio: '1/1', width: '100%', maxWidth: 460, marginLeft: 'auto',
          position: 'relative',
        }}>
          <img src="../../assets/bitmap.svg" alt=""
            style={{ width: '100%', height: '100%', objectFit: 'contain', opacity: 0.92 }} />
        </div>
      </div>
    </Container>
  </section>
);

const PositioningRow = () => (
  <section style={{ padding: '80px 0', borderTop: '1px solid var(--rbx-line)' }}>
    <Container>
      <Eyebrow style={{ marginBottom: 24 }}>Positioning</Eyebrow>
      <div style={{ display: 'grid', gridTemplateColumns: '1fr 1.6fr', gap: 64, alignItems: 'start' }}>
        <h2 style={{
          fontSize: 36, fontWeight: 400, lineHeight: 1.15, letterSpacing: '-0.015em',
          color: 'var(--rbx-fg)', margin: 0,
        }}>
          Systems designed<br />to operate.
        </h2>
        <div>
          <p style={{
            fontSize: 18, lineHeight: 1.55, fontWeight: 300,
            color: 'var(--rbx-fg)', margin: 0, maxWidth: '42rem',
          }}>
            We treat software as operational infrastructure. Every system we deliver is designed to be maintained, observed and evolved safely over years, not just to work on deployment day.
          </p>
          <p style={{
            marginTop: 20, fontSize: 15, lineHeight: 1.6,
            color: 'var(--rbx-fg-muted)', maxWidth: '42rem',
          }}>
            RBX designs and operates platforms, automations and infrastructure for environments where reliability, governance and control are requirements, not differentiators.
          </p>
        </div>
      </div>
    </Container>
  </section>
);

const Capabilities = () => {
  const items = [
    { n: '01', t: 'Architecture', d: 'Systems designed with scalable, observable and testable architecture from the start.' },
    { n: '02', t: 'Governance', d: 'Processes, standards and operational discipline integrated at every development phase.' },
    { n: '03', t: 'Operations', d: 'Continuous support, monitoring and safe evolution of production systems.' },
  ];
  return (
    <section style={{ padding: '80px 0', borderTop: '1px solid var(--rbx-line)' }}>
      <Container>
        <Eyebrow style={{ marginBottom: 48 }}>How we operate</Eyebrow>
        <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr 1fr', gap: 24 }}>
          {items.map(it => (
            <div key={it.n} style={{ borderLeft: '1px solid var(--rbx-line-strong)', paddingLeft: 24 }}>
              <div style={{
                fontFamily: 'var(--rbx-font-mono)', fontSize: 28, fontWeight: 300,
                color: 'var(--rbx-fg-faint)', letterSpacing: '0.04em', marginBottom: 16,
              }}>{it.n}</div>
              <h3 style={{ fontSize: 18, fontWeight: 500, color: 'var(--rbx-fg)', margin: '0 0 10px' }}>{it.t}</h3>
              <p style={{ fontSize: 13, lineHeight: 1.6, color: 'var(--rbx-fg-muted)', margin: 0 }}>{it.d}</p>
            </div>
          ))}
        </div>
      </Container>
    </section>
  );
};

const PRODUCTS = [
  { name: 'Robson', type: 'Fullstack', phase: 'Institutionalized', desc: 'Directional intelligence engine for algorithmic trading. Real-time market analysis, automated risk management, and strategy execution with AI.' },
  { name: 'Strategos', type: 'Fullstack', phase: 'Structuring', desc: 'Strategic operating system for cognitive organizational coordination. Unifies planning, execution and continuous learning.' },
  { name: 'TruthMetal', type: 'API', phase: 'Seed', desc: 'Canonical truth control plane for AI systems. Ensures parameters and decisions shared across agents remain auditable, versioned and immutable.' },
  { name: 'Thalamus', type: 'API', phase: 'Seed', desc: 'Routing and orchestration layer for analytical data. Central hub of signals and events across RBX products.' },
  { name: 'Argos Radar', type: 'Fullstack', phase: 'Seed', desc: 'Market surveillance platform. Detects patterns, anomalies and signals across assets and timeframes.' },
  { name: 'Eden', type: 'CLI', phase: 'Seed', desc: 'Internal developer platform. Automates provisioning of new products on Kubernetes with GitOps.' },
];

const Products = () => (
  <section style={{ padding: '96px 0', borderTop: '1px solid var(--rbx-line)' }}>
    <Container>
      <div style={{ marginBottom: 56, display: 'flex', justifyContent: 'space-between', alignItems: 'flex-end' }}>
        <div>
          <Eyebrow style={{ marginBottom: 12 }}>Open source · Made in Brazil</Eyebrow>
          <h2 style={{
            fontSize: 36, fontWeight: 400, lineHeight: 1.15, letterSpacing: '-0.015em',
            color: 'var(--rbx-fg)', margin: 0,
          }}>RBX Products</h2>
        </div>
        <p style={{ fontSize: 13, color: 'var(--rbx-fg-muted)', maxWidth: 340, margin: 0, lineHeight: 1.6 }}>
          All repositories are public. Built with operational rigor and ready for mission-critical deployments.
        </p>
      </div>
      <div style={{
        border: '1px solid var(--rbx-line)', borderRadius: 8, overflow: 'hidden',
      }}>
        {PRODUCTS.map((p, i) => (
          <div key={p.name} style={{
            display: 'grid', gridTemplateColumns: '200px 90px 1fr 180px',
            gap: 24, padding: '20px 24px', alignItems: 'center',
            borderTop: i === 0 ? 0 : '1px solid var(--rbx-line)',
            background: 'var(--rbx-surface-1)',
          }}>
            <div style={{ fontSize: 16, fontWeight: 500, color: 'var(--rbx-fg)', letterSpacing: '-0.01em' }}>{p.name}</div>
            <div style={{
              fontFamily: 'var(--rbx-font-mono)', fontSize: 10,
              color: 'var(--rbx-fg-dim)', letterSpacing: '0.04em', textTransform: 'uppercase',
            }}>{p.type}</div>
            <div style={{ fontSize: 13, color: 'var(--rbx-fg-muted)', lineHeight: 1.5, maxWidth: '56ch' }}>{p.desc}</div>
            <div style={{ display: 'flex', justifyContent: 'flex-end' }}>
              <PhaseTag phase={p.phase} />
            </div>
          </div>
        ))}
      </div>
    </Container>
  </section>
);

const Team = () => {
  const people = [
    { n: 'Rafael Scharf', r: 'Software Engineer Manager · Tech Lead', i: 'RS' },
    { n: 'Anthony Farias', r: 'Full Stack · Cybersecurity', i: 'AF' },
    { n: 'Leandro Damasio', r: 'CEO · Principal SWE · SRE · DevOps', i: 'LD' },
    { n: 'Magno Ozzyr', r: 'PM · Lean-Agile Delivery', i: 'MO' },
    { n: 'Flávia Ribeiro', r: 'SDR · Client Support', i: 'FR' },
  ];
  return (
    <section style={{ padding: '96px 0', borderTop: '1px solid var(--rbx-line)' }}>
      <Container>
        <div style={{ textAlign: 'center', marginBottom: 64 }}>
          <Eyebrow style={{ marginBottom: 20 }}>Our team</Eyebrow>
          <h2 style={{
            fontSize: 36, fontWeight: 400, lineHeight: 1.15, letterSpacing: '-0.015em',
            color: 'var(--rbx-fg)', margin: 0,
          }}>Engineers specialized in mission-critical systems.</h2>
        </div>
        <div style={{
          display: 'grid', gridTemplateColumns: 'repeat(5, 1fr)', gap: 16,
          padding: 32, borderRadius: 8,
          backgroundImage: 'url(../../assets/polka-dots.svg)',
          backgroundSize: '160px',
          border: '1px solid var(--rbx-line)',
        }}>
          {people.map(p => (
            <div key={p.n} style={{ display: 'flex', flexDirection: 'column', alignItems: 'center', gap: 12, padding: 16 }}>
              <div style={{
                width: 80, height: 80, borderRadius: '50%',
                background: 'var(--rbx-surface-3)', border: '1px solid var(--rbx-line)',
                display: 'flex', alignItems: 'center', justifyContent: 'center',
                fontFamily: 'var(--rbx-font-mono)', fontSize: 18, letterSpacing: '0.04em',
                color: 'var(--rbx-fg-muted)',
              }}>{p.i}</div>
              <div style={{ textAlign: 'center' }}>
                <div style={{ fontSize: 13, fontWeight: 500, color: 'var(--rbx-fg)' }}>{p.n}</div>
                <div style={{ fontSize: 11, color: 'var(--rbx-fg-dim)', marginTop: 4, lineHeight: 1.4 }}>{p.r}</div>
              </div>
            </div>
          ))}
        </div>
      </Container>
    </section>
  );
};

Object.assign(window, { Hero, PositioningRow, Capabilities, Products, Team });
