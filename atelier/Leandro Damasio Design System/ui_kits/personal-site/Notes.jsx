// Notes.jsx — Notes index + Note detail

const NOTES_LIST = [
  { id: 1, title: "Prompt governance in regulated AI: lessons from financial-sector deployments", date: "2026-02", tag: "AI · Governance", readTime: "8 min" },
  { id: 2, title: "On agentic runtimes: control loops, memory, and the limits of LLM autonomy", date: "2025-11", tag: "AI · Systems", readTime: "11 min" },
  { id: 3, title: "Vector search at the edge of compliance: pgvector, ParadeDB, and auditability", date: "2025-08", tag: "Infrastructure", readTime: "7 min" },
  { id: 4, title: "Governing AI in high-reliability systems: a practitioner's notes", date: "2025-04", tag: "AI · Architecture", readTime: "9 min" },
  { id: 5, title: "GitOps for AI workloads: ArgoCD, k3s, and the reproducibility problem", date: "2025-01", tag: "DevOps", readTime: "6 min" },
];

const NOTE_BODY = `AI systems deployed in financial and legal environments operate under a constraint that general-purpose LLM products do not face: every decision the system influences must be traceable, auditable, and defensible.

This is not a soft requirement. In regulated environments, the absence of auditability is a compliance failure. The system does not get a second chance.

What follows are practical notes from building such systems, with specific attention to prompt governance: how prompts are versioned, tested, promoted, and monitored in production.

---

**The prompt is an artifact, not a string**

The most important shift in mindset is treating prompts as versioned artifacts rather than strings. A prompt has a lineage. It was written by someone, reviewed by someone, tested against a dataset, and promoted through an environment pipeline.

When a model produces an unexpected output, the question is not "what did the model do?" It is: "which prompt version, with which model version, against which context, produced this output?"

Without versioning, that question has no answer.

---

**Governance surfaces**

At Enforce, we built a platform (Forja) that centralizes prompt and workflow governance. The key surfaces:

- **Prompt registry:** every prompt has an ID, version, author, and status (draft, testing, staging, production, deprecated).
- **Evaluation suite:** each promotion runs the prompt against a curated dataset with expected outputs. Regression gates are mandatory.
- **Context audit log:** every inference call logs the prompt version, model, context hash, and output. Retained for 90 days minimum.
- **Rollback mechanism:** production can revert to the previous prompt version in under 60 seconds.

These are not nice-to-haves. They are the foundation.

---

**Metrics**

\`\`\`
Time to test new AI workflow:  days → hours (after Forja)
Prompt versions in production: 14 (across 4 internal systems)
Regression gate pass rate:     97.3%
Mean time to rollback:         <60s
\`\`\`

The numbers matter less than the fact that they exist. An ungoverned system cannot produce them.`;

function NotesPage({ note, onNav, theme }) {
  const isLight = theme === 'light';
  const fg  = isLight ? '#0A0A0B' : '#F5F5F3';
  const fg2 = isLight ? '#3A3B3E' : '#B4B5B8';
  const fg3 = isLight ? '#6A6B6E' : '#72747A';
  const bg  = isLight ? '#F5F5F3' : '#0A0A0B';
  const hr  = isLight ? '#C8C8C4' : '#26272C';
  const [openNote, setOpenNote] = React.useState(note);

  if (openNote) {
    return (
      <div style={{ background: bg, minHeight: '100vh', fontFamily: "'Geist',sans-serif" }}>
        <Nav page="notes" onNav={onNav} theme={theme} onToggleTheme={() => {}} />

        <div style={{ maxWidth: 672, margin: '0 auto', padding: '100px 32px 80px' }}>
          <button onClick={() => setOpenNote(null)} style={{ fontFamily: "'GeistMono',monospace", fontSize: 10, letterSpacing: '0.08em', color: fg3, background: 'none', border: 'none', cursor: 'pointer', padding: 0, marginBottom: 40 }}>
            ← NOTES
          </button>

          <div style={{ borderTop: `1px solid ${hr}`, paddingTop: 16, marginBottom: 32 }}>
            <div style={{ display: 'flex', gap: 10, marginBottom: 10, flexWrap: 'wrap' }}>
              <span style={{ fontFamily: "'GeistMono',monospace", fontSize: 9, color: fg3 }}>{openNote.date || '2026-02'}</span>
              <span style={{ fontFamily: "'GeistMono',monospace", fontSize: 9, color: '#5C7080' }}>{openNote.tag || 'AI · Governance'}</span>
              <span style={{ fontFamily: "'GeistMono',monospace", fontSize: 9, color: fg3 }}>{openNote.readTime || '8 min'}</span>
            </div>
            <h1 style={{ fontFamily: "'Geist',sans-serif", fontSize: 'clamp(22px, 3.5vw, 32px)', fontWeight: 300, color: fg, lineHeight: 1.2, letterSpacing: '-0.01em' }}>
              {openNote.title}
            </h1>
          </div>

          <div style={{ fontFamily: "'Geist',sans-serif", fontSize: 16, color: fg2, lineHeight: 1.8 }}>
            {NOTE_BODY.split('\n\n').map((para, i) => {
              if (para.startsWith('---')) return <hr key={i} style={{ border: 'none', borderTop: `1px solid ${hr}`, margin: '28px 0' }} />;
              if (para.startsWith('**') && para.endsWith('**')) return <h2 key={i} style={{ fontFamily: "'Geist',sans-serif", fontSize: 18, fontWeight: 400, color: fg, marginBottom: 12, marginTop: 28 }}>{para.replace(/\*\*/g, '')}</h2>;
              if (para.startsWith('```')) {
                const code = para.replace(/```\n?/g, '');
                return <pre key={i} style={{ fontFamily: "'GeistMono',monospace", fontSize: 11, color: fg2, background: isLight ? '#E8E8E6' : '#15161A', border: `1px solid ${hr}`, borderRadius: 6, padding: '14px 16px', margin: '20px 0', overflow: 'auto', lineHeight: 1.8 }}>{code}</pre>;
              }
              if (para.startsWith('- **')) {
                return <ul key={i} style={{ listStyle: 'none', padding: 0, marginBottom: 16 }}>{
                  para.split('\n').map((li, j) => {
                    const match = li.match(/- \*\*(.+?)\*\*: (.+)/);
                    if (!match) return null;
                    return <li key={j} style={{ marginBottom: 8, paddingLeft: 16, borderLeft: `2px solid #26272C` }}>
                      <span style={{ fontFamily: "'GeistMono',monospace", fontSize: 11, color: fg, fontWeight: 500 }}>{match[1]}:</span>
                      <span style={{ fontFamily: "'Geist',sans-serif", fontSize: 14, color: fg2 }}> {match[2]}</span>
                    </li>;
                  })
                }</ul>;
              }
              return <p key={i} style={{ marginBottom: 20 }}>{para}</p>;
            })}
          </div>
        </div>

        <footer style={{ background: isLight ? '#DDDDD9' : '#1D1D22', borderTop: `1px solid ${hr}`, padding: '16px 32px', marginTop: 40 }}>
          <div style={{ maxWidth: 1152, margin: '0 auto' }}>
            <span style={{ fontFamily: "'GeistMono',monospace", fontSize: 9, letterSpacing: '0.12em', textTransform: 'uppercase', color: fg3 }}>RBX Systems · CHE-xxx.xxx.xxx</span>
          </div>
        </footer>
      </div>
    );
  }

  // Notes index
  return (
    <div style={{ background: bg, minHeight: '100vh', fontFamily: "'Geist',sans-serif" }}>
      <Nav page="notes" onNav={onNav} theme={theme} onToggleTheme={() => {}} />

      <div style={{ maxWidth: 720, margin: '0 auto', padding: '100px 32px 80px' }}>
        <div style={{ borderTop: `1px solid ${hr}`, paddingTop: 16, marginBottom: 32 }}>
          <div style={{ fontFamily: "'GeistMono',monospace", fontSize: 10, fontWeight: 500, letterSpacing: '0.14em', textTransform: 'uppercase', color: fg3, marginBottom: 8 }}>Writing</div>
          <h1 style={{ fontFamily: "'Geist',sans-serif", fontSize: 36, fontWeight: 300, color: fg, lineHeight: 1.1, letterSpacing: '-0.02em' }}>Notes</h1>
        </div>

        <div>
          {NOTES_LIST.map((n, i) => (
            <div key={n.id} onClick={() => setOpenNote(n)}
              style={{ display: 'grid', gridTemplateColumns: '1fr auto', alignItems: 'baseline', gap: 16, padding: '14px 0', borderBottom: `1px solid ${hr}`, cursor: 'pointer' }}>
              <div>
                <div style={{ fontFamily: "'GeistMono',monospace", fontSize: 9, color: '#5C7080', marginBottom: 4 }}>{n.tag}</div>
                <div style={{ fontFamily: "'Geist',sans-serif", fontSize: 15, color: fg2, lineHeight: 1.45 }}>{n.title}</div>
              </div>
              <div style={{ display: 'flex', flexDirection: 'column', alignItems: 'flex-end', gap: 3 }}>
                <span style={{ fontFamily: "'GeistMono',monospace", fontSize: 10, color: fg3, whiteSpace: 'nowrap' }}>{n.date}</span>
                <span style={{ fontFamily: "'GeistMono',monospace", fontSize: 9, color: fg3 }}>{n.readTime}</span>
              </div>
            </div>
          ))}
        </div>
      </div>

      <footer style={{ background: isLight ? '#DDDDD9' : '#1D1D22', borderTop: `1px solid ${hr}`, padding: '16px 32px', marginTop: 40 }}>
        <div style={{ maxWidth: 1152, margin: '0 auto' }}>
          <span style={{ fontFamily: "'GeistMono',monospace", fontSize: 9, letterSpacing: '0.12em', textTransform: 'uppercase', color: fg3 }}>RBX Systems · CHE-xxx.xxx.xxx</span>
        </div>
      </footer>
    </div>
  );
}

Object.assign(window, { NotesPage });
