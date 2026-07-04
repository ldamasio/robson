// Home.jsx — Hero + Selected Work + Capabilities + Contact

const WORKS = [
  { id: 1, client: "Enforce / BTG Pactual Group", role: "AI Engineer", years: "2025–present", tech: "Python · FastAPI · k3s · ArgoCD · pgvector · RAG", line: "Production-grade AI systems for financial and legal domains. RAG pipelines, prompt governance, agentic architectures.", status: "active", statusLabel: "Active" },
  { id: 2, client: "RBX Systems", role: "Founder", years: "2020–present", tech: "Go · Rust · TypeScript · Kubernetes · Svelte", line: "AI agents, trading systems, decision infrastructure, and internal developer platform.", status: "active", statusLabel: "Active" },
  { id: 3, client: "Arte Arena", role: "Principal Software Engineer", years: "2024–2025", tech: "Go · Python · FastAPI · React · Kubernetes · AWS", line: "Modernized legacy Laravel platform to cloud-native SaaS. 42% faster deploys, 39% compute cost reduction.", status: "shipped", statusLabel: "Shipped" },
  { id: 4, client: "Stefanini / FAPESP", role: "Senior Software Engineer", years: "2024–2025", tech: "Django · Laravel · Apache Solr", line: "Search engine overhaul for FAPESP Virtual Library. 37% query response improvement.", status: "shipped", statusLabel: "Shipped" },
  { id: 5, client: "Tier-1 Brazilian financial group", role: "Software Engineer", years: "2019–2022", tech: "Node.js · AWS · Ethereum · Smart Contracts", line: "Automated data pipelines and smart contract infrastructure for distressed credit operations.", status: "shipped", statusLabel: "Shipped" },
  { id: 6, client: "Global Hitss", role: "Software Engineer", years: "2017–2019", tech: "React · Nest.js · Hadoop · Spark · Docker", line: "Scalable data lake and full-stack delivery for enterprise financial sector clients.", status: "shipped", statusLabel: "Shipped" },
];

const PERSONAL_TOOLS = [
  { id: 7, client: "RTK · Rust Token Killer", role: "Author", years: "2024", tech: "Rust", line: "Token-level audit tool for LLM prompt chains. Detects leakage and governance violations.", status: "production", statusLabel: "In production" },
  { id: 8, client: "x.sh", role: "Author", years: "2023–present", tech: "Bash · JSON", line: "Governed execution runtime: turns shell commands into durable, machine-readable traces for LLMs.", status: "active", statusLabel: "Active" },
  { id: 9, client: "wt", role: "Author", years: "2023–present", tech: "Bash · Git", line: "CLI for Git worktree management with environment profiles. Rapid context switching.", status: "active", statusLabel: "Active" },
  { id: 10, client: "Strategos", role: "Author", years: "2022–present", tech: "Go · Svelte · PostgreSQL · MongoDB", line: "Situation room interface for human-AI strategic deliberation and governed decision review.", status: "active", statusLabel: "Active" },
];

const CAPABILITIES = `AI/LLM systems · RAG pipelines · agentic runtimes · prompt governance
distributed systems · trading infrastructure · event-driven architecture
Rust · TypeScript · Python · Go · Bash
Kubernetes · k3s · ArgoCD · GitOps · Docker
PostgreSQL · pgvector · ParadeDB · MongoDB · Redis
FastAPI · Next.js · React · Svelte · Nest.js
observability · MLOps · CI/CD · cloud-native (AWS · GCP)
vector search · embeddings · LLM evaluation · fine-tuning`;

const statusColor = {
  active:      { color: '#7A93B0', border: '#7A93B0', bg: 'rgba(122,147,176,0.08)' },
  shipped:     { color: '#7FB77E', border: '#7FB77E', bg: 'rgba(127,183,126,0.08)' },
  production:  { color: '#7FB77E', border: '#7FB77E', bg: 'rgba(127,183,126,0.08)' },
  progress:    { color: '#D9B55A', border: '#D9B55A', bg: 'rgba(217,181,90,0.08)'  },
};

function StatusPill({ status, label }) {
  const s = statusColor[status] || statusColor.active;
  return (
    <span style={{
      fontFamily: "'GeistMono', monospace", fontSize: 9, letterSpacing: '0.04em',
      textTransform: 'uppercase', padding: '3px 9px', borderRadius: 999,
      border: `1px solid ${s.border}`, color: s.color, background: s.bg,
      whiteSpace: 'nowrap',
    }}>● {label}</span>
  );
}

function Eyebrow({ children }) {
  return (
    <div style={{
      fontFamily: "'GeistMono','Geist',sans-serif", fontSize: 10, fontWeight: 500,
      letterSpacing: '0.14em', textTransform: 'uppercase', color: '#72747A',
      borderTop: '1px solid #26272C', paddingTop: 12, marginBottom: 8,
    }}>{children}</div>
  );
}

function WorkCard({ work, onNav, fg }) {
  const [hov, setHov] = React.useState(false);
  return (
    <div
      onMouseEnter={() => setHov(true)}
      onMouseLeave={() => setHov(false)}
      onClick={() => onNav('work', { work })}
      style={{
        background: hov ? '#1D1D22' : '#15161A',
        border: `1px solid ${hov ? '#5C7080' : '#26272C'}`,
        borderRadius: 8, padding: '16px 20px', cursor: 'pointer',
        boxShadow: 'inset 0 1px 0 rgba(255,255,255,0.04), 0 1px 3px rgba(0,0,0,0.5)',
        transition: 'border-color 160ms, background 160ms',
      }}
    >
      <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'flex-start', gap: 12, marginBottom: 4 }}>
        <div>
          <div style={{ fontFamily: "'Geist',sans-serif", fontSize: 10, fontWeight: 500, letterSpacing: '0.1em', textTransform: 'uppercase', color: '#72747A', marginBottom: 3 }}>{work.client}</div>
          <div style={{ fontFamily: "'Geist',sans-serif", fontSize: 15, fontWeight: 400, color: fg || '#F5F5F3' }}>{work.role}</div>
        </div>
        <StatusPill status={work.status} label={work.statusLabel} />
      </div>
      <div style={{ fontFamily: "'GeistMono',monospace", fontSize: 10, color: '#72747A', marginBottom: 6 }}>{work.years}</div>
      <div style={{ fontFamily: "'Geist',sans-serif", fontSize: 13, color: '#B4B5B8', lineHeight: 1.55, marginBottom: 10 }}>{work.line}</div>
      <div style={{ fontFamily: "'GeistMono',monospace", fontSize: 9, color: '#45474D' }}>{work.tech}</div>
    </div>
  );
}

function Nav({ page, onNav, theme, onToggleTheme }) {
  const items = [
    { id: 'home', label: 'Home' },
    { id: 'work', label: 'Work' },
    { id: 'notes', label: 'Notes' },
  ];
  const [anchor, setAnchor] = React.useState(null);
  return (
    <div style={{ position: 'fixed', top: 0, left: 0, right: 0, zIndex: 50, display: 'flex', justifyContent: 'center', padding: '16px 16px 0', pointerEvents: 'none' }}>
      <nav style={{
        display: 'inline-flex', alignItems: 'center', gap: 2,
        background: 'rgba(10,10,11,0.85)', backdropFilter: 'blur(12px)', WebkitBackdropFilter: 'blur(12px)',
        border: '1px solid rgba(38,39,44,0.7)', borderRadius: 999,
        padding: '5px 8px', boxShadow: '0 4px 24px rgba(0,0,0,0.6)',
        pointerEvents: 'auto',
      }}>
        {items.map(item => {
          const active = page === item.id || (item.id === 'work' && page === 'work') || (item.id === 'notes' && page === 'notes');
          return (
            <button key={item.id} onClick={() => onNav(item.id)}
              style={{
                fontFamily: "'Geist',sans-serif", fontSize: 13, fontWeight: 500,
                color: active ? '#F5F5F3' : '#72747A',
                background: active ? '#1D1D22' : 'transparent',
                border: 'none', cursor: 'pointer', borderRadius: 999,
                padding: '5px 14px', transition: 'color 160ms, background 160ms',
              }}
            >{item.label}</button>
          );
        })}
        <div style={{ width: 1, height: 16, background: '#26272C', margin: '0 4px' }}></div>
        <button onClick={onToggleTheme} style={{
          fontFamily: "'GeistMono',monospace", fontSize: 10, color: '#72747A',
          background: 'transparent', border: 'none', cursor: 'pointer', padding: '5px 10px', borderRadius: 999,
        }}>{theme === 'dark' ? 'EN' : 'PT'}</button>
      </nav>
    </div>
  );
}

function HomePage({ onNav, theme, onToggleTheme }) {
  const isLight = theme === 'light';
  const fg  = isLight ? '#0A0A0B' : '#F5F5F3';
  const fg2 = isLight ? '#3A3B3E' : '#B4B5B8';
  const fg3 = isLight ? '#6A6B6E' : '#72747A';
  const bg  = isLight ? '#F5F5F3' : '#0A0A0B';
  const cardBg = isLight ? '#E8E8E6' : '#15161A';
  const hr  = isLight ? '#C8C8C4' : '#26272C';

  return (
    <div style={{ background: bg, minHeight: '100vh', fontFamily: "'Geist',sans-serif" }}>
      <Nav page="home" onNav={onNav} theme={theme} onToggleTheme={onToggleTheme} />

      {/* Hero */}
      <section style={{ maxWidth: 1152, margin: '0 auto', padding: '120px 32px 80px' }}>
        <div style={{ display: 'flex', alignItems: 'flex-start', gap: 32, flexWrap: 'wrap' }}>
          <div style={{ flex: 1, minWidth: 280 }}>
            <div style={{ fontFamily: "'GeistMono',monospace", fontSize: 10, fontWeight: 500, letterSpacing: '0.14em', textTransform: 'uppercase', color: fg3, marginBottom: 16 }}>Brazil · Zürich</div>
            <h1 style={{ fontFamily: "'Geist',sans-serif", fontSize: 'clamp(36px, 5vw, 64px)', fontWeight: 300, color: fg, lineHeight: 1.05, letterSpacing: '-0.02em', marginBottom: 20 }}>
              Leandro Damasio
            </h1>
            <p style={{ fontFamily: "'Geist',sans-serif", fontSize: 18, fontWeight: 400, color: fg2, lineHeight: 1.6, maxWidth: 540, marginBottom: 12 }}>
              Computer Engineer. AI systems for finance and high-reliability environments.
            </p>
            <p style={{ fontFamily: "'Geist',sans-serif", fontSize: 15, color: fg3, lineHeight: 1.6, maxWidth: 480, marginBottom: 28 }}>
              Based in Brazil, working across Zürich and São Paulo. EN · PT · DE basic.
            </p>
            <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap' }}>
              <button onClick={() => onNav('work')} style={{ fontFamily: "'Geist',sans-serif", fontSize: 13, fontWeight: 500, color: fg, background: 'transparent', border: `1px solid ${hr}`, borderRadius: 6, padding: '8px 18px', cursor: 'pointer', transition: 'border-color 160ms' }}>Selected work</button>
              <button onClick={() => onNav('notes')} style={{ fontFamily: "'Geist',sans-serif", fontSize: 13, fontWeight: 500, color: fg3, background: 'transparent', border: 'none', padding: '8px 4px', cursor: 'pointer' }}>Notes</button>
            </div>
          </div>
          {/* Portrait placeholder */}
          <div style={{ width: 96, height: 96, border: `1px solid ${hr}`, borderRadius: 2, background: isLight ? '#DDDDD9' : '#1D1D22', flexShrink: 0, display: 'flex', alignItems: 'center', justifyContent: 'center' }}>
            <div style={{ width: 40, height: 40, borderRadius: '50%', background: isLight ? '#B0B0AA' : '#33343A' }}></div>
          </div>
        </div>
      </section>

      {/* Selected Work */}
      <section style={{ maxWidth: 1152, margin: '0 auto', padding: '0 32px 80px' }}>
        <Eyebrow>Selected Projects</Eyebrow>
        <h2 style={{ fontFamily: "'Geist',sans-serif", fontSize: 28, fontWeight: 300, color: fg, marginBottom: 24, letterSpacing: '-0.01em' }}>Work, 2018–2026</h2>
        <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fill, minmax(300px, 1fr))', gap: 12, marginBottom: 40 }}>
          {WORKS.map(w => <WorkCard key={w.id} work={w} onNav={onNav} fg={fg} />)}
        </div>

        <Eyebrow>Personal Tools</Eyebrow>
        <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fill, minmax(300px, 1fr))', gap: 12 }}>
          {PERSONAL_TOOLS.map(w => <WorkCard key={w.id} work={w} onNav={onNav} fg={fg} />)}
        </div>
      </section>

      {/* Capabilities */}
      <section style={{ maxWidth: 1152, margin: '0 auto', padding: '0 32px 80px' }}>
        <Eyebrow>Capabilities</Eyebrow>
        <div style={{ fontFamily: "'GeistMono',monospace", fontSize: 12, color: fg2, lineHeight: 2, whiteSpace: 'pre-line', maxWidth: 720 }}>{CAPABILITIES}</div>
      </section>

      {/* Writing */}
      <section style={{ maxWidth: 1152, margin: '0 auto', padding: '0 32px 80px' }}>
        <Eyebrow>Writing</Eyebrow>
        {[
          { title: "Prompt governance in regulated AI: lessons from financial-sector deployments", date: "2026-02" },
          { title: "On agentic runtimes: control loops, memory, and the limits of LLM autonomy", date: "2025-11" },
          { title: "Vector search at the edge of compliance: pgvector, ParadeDB, and auditability", date: "2025-08" },
          { title: "Governing AI in high-reliability systems: a practitioner's notes", date: "2025-04" },
        ].map((n, i) => (
          <div key={i} onClick={() => onNav('notes', { note: n })}
            style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'baseline', gap: 16, padding: '10px 0', borderBottom: `1px solid ${hr}`, cursor: 'pointer' }}>
            <span style={{ fontFamily: "'Geist',sans-serif", fontSize: 14, color: fg2, lineHeight: 1.4 }}>{n.title}</span>
            <span style={{ fontFamily: "'GeistMono',monospace", fontSize: 10, color: fg3, whiteSpace: 'nowrap' }}>{n.date}</span>
          </div>
        ))}
      </section>

      {/* Contact */}
      <section style={{ maxWidth: 1152, margin: '0 auto', padding: '0 32px 80px' }}>
        <Eyebrow>Contact</Eyebrow>
        <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
          <a href="mailto:leandro@rbxsystems.ch" style={{ fontFamily: "'GeistMono',monospace", fontSize: 13, color: fg2, textDecoration: 'none', borderBottom: `1px solid ${hr}`, paddingBottom: 2, width: 'fit-content' }}>leandro@rbxsystems.ch</a>
          <a href="#" style={{ fontFamily: "'GeistMono',monospace", fontSize: 12, color: fg3, textDecoration: 'none' }}>github.com/ldamasio</a>
          <a href="#" style={{ fontFamily: "'GeistMono',monospace", fontSize: 12, color: fg3, textDecoration: 'none' }}>linkedin.com/in/ldamasio</a>
        </div>
        <p style={{ fontFamily: "'Geist',sans-serif", fontSize: 12, color: fg3, marginTop: 16 }}>Available for selected engagements.</p>
      </section>

      {/* Footer */}
      <footer style={{ background: isLight ? '#DDDDD9' : '#1D1D22', borderTop: `1px solid ${hr}`, padding: '16px 32px' }}>
        <div style={{ maxWidth: 1152, margin: '0 auto', display: 'flex', justifyContent: 'space-between', alignItems: 'center', flexWrap: 'wrap', gap: 8 }}>
          <div style={{ display: 'flex', flexDirection: 'column', gap: 3 }}>
            <span style={{ fontFamily: "'GeistMono',monospace", fontSize: 9, letterSpacing: '0.12em', textTransform: 'uppercase', color: fg3 }}>RBX Systems · CHE-xxx.xxx.xxx</span>
            <span style={{ fontFamily: "'Geist',sans-serif", fontSize: 10, color: fg3 }}>© 2026 Leandro Damasio. All rights reserved.</span>
          </div>
          <div style={{ display: 'flex', gap: 10 }}>
            <button onClick={onToggleTheme} style={{ fontFamily: "'GeistMono',monospace", fontSize: 9, color: fg3, background: 'none', border: 'none', cursor: 'pointer', letterSpacing: '0.08em' }}>EN / PT-BR</button>
          </div>
        </div>
      </footer>
    </div>
  );
}

Object.assign(window, { HomePage, WorkCard, StatusPill, Eyebrow, Nav, WORKS, PERSONAL_TOOLS, statusColor });
