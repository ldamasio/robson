// Top nav + footer shared across marketing + atelier surfaces.

const TopNav = ({ active = 'home', onNav }) => {
  const [menu, setMenu] = useState(null);
  const items = [
    { id: 'about', label: 'About Us', sub: [
      { t: 'Who We Are', d: 'Systems engineering, automation and infrastructure for high-demand operations.', highlight: true },
      { t: 'Positioning', d: 'How we think about technology, operations and reliability.' },
      { t: 'Approach', d: 'Architecture, governance and operational discipline.' },
      { t: 'Team', d: 'Engineers with experience in critical systems and infrastructure.' },
    ]},
    { id: 'services', label: 'Services', sub: [
      { t: 'Web Systems & Platforms', d: 'Applications and internal platforms for continuous operation.' },
      { t: 'Automation & Integrations', d: 'Workflows for critical flows.' },
      { t: 'Applied AI & Agents', d: 'Agents integrated with governance and observability.' },
      { t: 'Cloud Infrastructure', d: 'Declarative provisioning, CI/CD, reliable operations.' },
      { t: 'Backend & APIs', d: 'Services designed for consistency and performance.' },
      { t: 'Evolutionary Maintenance', d: 'Continuous evolution with traceability.' },
    ]},
    { id: 'products', label: 'Products' },
    { id: 'blog', label: 'Blog' },
    { id: 'contact', label: 'Contact' },
    { id: 'atelier', label: 'Atelier', strong: true },
  ];

  return (
    <div style={{
      position: 'sticky', top: 16, zIndex: 50,
      margin: '16px auto 0', maxWidth: 1280, padding: '0 24px',
    }}>
      <div style={{
        display: 'flex', alignItems: 'center', justifyContent: 'space-between',
        gap: 16, padding: '10px 16px',
        background: 'rgba(10,10,11,0.75)',
        backdropFilter: 'blur(12px)', WebkitBackdropFilter: 'blur(12px)',
        border: '1px solid var(--rbx-line)', borderRadius: 12,
      }}>
        <a onClick={() => onNav && onNav('home')} style={{ border: 0, cursor: 'pointer' }}>
          <RbxLogo />
        </a>

        <nav style={{ display: 'flex', alignItems: 'center', gap: 4 }}>
          {items.map(it => (
            <div key={it.id}
              onMouseEnter={() => it.sub && setMenu(it.id)}
              onMouseLeave={() => setMenu(null)}
              style={{ position: 'relative' }}>
              <button
                onClick={() => onNav && onNav(it.id)}
                style={{
                  background: 'transparent', border: 0, cursor: 'pointer',
                  padding: '8px 14px', fontFamily: 'var(--rbx-font-sans)',
                  fontSize: 13, fontWeight: 500,
                  color: active === it.id ? 'var(--rbx-fg)'
                       : it.strong ? 'var(--rbx-accent)' : 'var(--rbx-fg-muted)',
                  letterSpacing: it.strong ? '0.02em' : 0,
                  transition: 'color 200ms var(--rbx-ease)',
                  display: 'flex', alignItems: 'center', gap: 4,
                }}
                onMouseEnter={e => e.currentTarget.style.color = 'var(--rbx-fg)'}
                onMouseLeave={e => e.currentTarget.style.color = active === it.id ? 'var(--rbx-fg)' : it.strong ? 'var(--rbx-accent)' : 'var(--rbx-fg-muted)'}
              >
                {it.label}
                {it.sub && <Icon name="chevron" size={12} />}
              </button>
              {menu === it.id && it.sub && (
                <div style={{
                  position: 'absolute', top: '100%', left: 0,
                  marginTop: 8, width: 440, padding: 8,
                  background: 'var(--rbx-ink)', border: '1px solid var(--rbx-line)',
                  borderRadius: 8, boxShadow: '0 24px 48px -16px rgba(0,0,0,0.6)',
                  display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 4,
                }}>
                  {it.sub.map((s, i) => (
                    <a key={i} style={{
                      padding: 12, borderRadius: 6, border: 0, cursor: 'pointer',
                      background: s.highlight ? 'var(--rbx-surface-2)' : 'transparent',
                      gridColumn: s.highlight ? 'span 2' : 'auto',
                      display: 'block',
                    }}
                    onMouseEnter={e => !s.highlight && (e.currentTarget.style.background = 'var(--rbx-surface-1)')}
                    onMouseLeave={e => !s.highlight && (e.currentTarget.style.background = 'transparent')}
                    >
                      <div style={{ fontSize: 13, fontWeight: 500, color: 'var(--rbx-fg)', marginBottom: 4 }}>{s.t}</div>
                      <div style={{ fontSize: 12, color: 'var(--rbx-fg-dim)', lineHeight: 1.45 }}>{s.d}</div>
                    </a>
                  ))}
                </div>
              )}
            </div>
          ))}
        </nav>

        <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
          <button style={{
            background: 'transparent', border: '1px solid var(--rbx-line)',
            color: 'var(--rbx-fg-muted)', padding: '6px 10px',
            fontFamily: 'var(--rbx-font-mono)', fontSize: 10,
            letterSpacing: '0.04em', textTransform: 'uppercase',
            borderRadius: 4, cursor: 'pointer',
          }}>EN</button>
          <Button variant="outline" size="sm">Contact</Button>
        </div>
      </div>
    </div>
  );
};

const Footer = () => (
  <footer style={{
    marginTop: 128, background: 'var(--rbx-surface-3)',
    borderTop: '1px solid var(--rbx-line)', padding: '64px 0 32px',
  }}>
    <Container>
      <div style={{ display: 'grid', gridTemplateColumns: '1.3fr 1fr 1fr 1fr', gap: 48 }}>
        <div>
          <RbxLogo markSize={40} />
          <p style={{
            marginTop: 20, fontSize: 13, lineHeight: 1.6,
            color: 'var(--rbx-fg-dim)', maxWidth: 280,
          }}>
            Systems engineering, automation and infrastructure for high-demand operations.
          </p>
        </div>
        {[
          { title: 'Services', links: ['Web Systems & Platforms', 'Automation & Integrations', 'Applied AI & Agents', 'Cloud Infrastructure', 'Backend & APIs', 'Evolutionary Maintenance'] },
          { title: 'Company', links: ['About us', 'Team', 'Blog', 'Products', 'Atelier'] },
          { title: 'Contact', links: ['contato@rbx.ia.br'] },
        ].map((sec, i) => (
          <div key={i}>
            <div style={{
              fontFamily: 'var(--rbx-font-mono)', fontSize: 10,
              textTransform: 'uppercase', letterSpacing: '0.14em',
              color: 'var(--rbx-fg-dim)', marginBottom: 20,
            }}>{sec.title}</div>
            <ul style={{ listStyle: 'none', padding: 0, margin: 0, display: 'flex', flexDirection: 'column', gap: 10 }}>
              {sec.links.map((l, j) => (
                <li key={j}>
                  <a style={{ fontSize: 13, color: 'var(--rbx-fg-muted)', border: 0, cursor: 'pointer' }}>{l}</a>
                </li>
              ))}
            </ul>
          </div>
        ))}
      </div>
      <div style={{
        marginTop: 64, paddingTop: 24, borderTop: '1px solid var(--rbx-line)',
        display: 'flex', justifyContent: 'space-between', alignItems: 'center',
      }}>
        <div style={{
          fontFamily: 'var(--rbx-font-mono)', fontSize: 11,
          color: 'var(--rbx-fg-dim)', letterSpacing: '0.04em',
        }}>© 2026 RBX SYSTEMS · ALL RIGHTS RESERVED</div>
        <div style={{ display: 'flex', gap: 16, color: 'var(--rbx-fg-dim)' }}>
          <Icon name="github" size={18} /><Icon name="linkedin" size={18} />
        </div>
      </div>
    </Container>
  </footer>
);

Object.assign(window, { TopNav, Footer });
