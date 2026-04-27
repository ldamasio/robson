// Shared RBX primitives. Babel JSX. Exports to window so other files can use them.
// Depends on colors_and_type.css being loaded on the page.

const { useState, useEffect, useRef, useMemo } = React;

// ——— Layout primitives ———
const Container = ({ children, style, max = 1152 }) => (
  <div style={{ maxWidth: max, margin: '0 auto', padding: '0 24px', ...style }}>{children}</div>
);

const Hair = ({ style }) => (
  <div style={{ borderTop: '1px solid var(--rbx-line)', ...style }} />
);

const Eyebrow = ({ children, style }) => (
  <div style={{
    fontSize: 11, textTransform: 'uppercase', letterSpacing: '0.14em',
    color: 'var(--rbx-fg-dim)', fontWeight: 500, ...style
  }}>{children}</div>
);

// ——— Buttons ———
const Button = ({ children, variant = 'primary', size = 'md', onClick, href, style, ...rest }) => {
  const base = {
    display: 'inline-flex', alignItems: 'center', justifyContent: 'center',
    gap: 8, whiteSpace: 'nowrap', cursor: 'pointer',
    fontFamily: 'var(--rbx-font-sans)', fontWeight: 500,
    transition: 'background 200ms var(--rbx-ease), border-color 200ms var(--rbx-ease), color 200ms var(--rbx-ease), opacity 120ms',
    borderRadius: 6, border: '1px solid transparent',
    ...(size === 'sm' ? { padding: '6px 12px', fontSize: 12 }
       : size === 'lg' ? { padding: '14px 22px', fontSize: 14 }
       : { padding: '10px 18px', fontSize: 13 }),
  };
  const variants = {
    primary: { background: 'var(--rbx-fg)', color: 'var(--rbx-ink)' },
    outline: { background: 'transparent', color: 'var(--rbx-fg)', borderColor: 'var(--rbx-line-strong)' },
    ghost: { background: 'transparent', color: 'var(--rbx-fg-muted)', padding: '10px 8px', border: 0 },
    accent: { background: 'var(--rbx-accent)', color: 'var(--rbx-ink)' },
  };
  const Tag = href ? 'a' : 'button';
  return (
    <Tag href={href} onClick={onClick} style={{ ...base, ...(variants[variant] || {}), ...style }} {...rest}>
      {children}
    </Tag>
  );
};

// ——— Status dot ———
const StatusDot = ({ color = 'var(--rbx-ok)', size = 6 }) => (
  <span style={{
    display: 'inline-block', width: size, height: size,
    background: color, borderRadius: '50%'
  }} />
);

// ——— Status pill ———
const StatusPill = ({ state, children, tone = 'ok' }) => {
  const colors = {
    ok: 'var(--rbx-ok)', warn: 'var(--rbx-warn)',
    err: 'var(--rbx-err)', info: 'var(--rbx-info)',
    neutral: 'var(--rbx-fg-muted)',
  };
  return (
    <span style={{
      display: 'inline-flex', alignItems: 'center', gap: 8,
      padding: '4px 10px', border: '1px solid var(--rbx-line)',
      borderRadius: 999, background: 'var(--rbx-surface-1)',
      fontFamily: 'var(--rbx-font-mono)', fontSize: 10,
      letterSpacing: '0.04em', color: 'var(--rbx-fg)',
      textTransform: 'uppercase',
    }}>
      <StatusDot color={colors[tone]} />
      {state || children}
    </span>
  );
};

// ——— Phase tag ———
const PhaseTag = ({ phase }) => {
  const highlight = phase === 'Institutionalized';
  return (
    <span style={{
      fontFamily: 'var(--rbx-font-mono)', fontSize: 10,
      letterSpacing: '0.04em', textTransform: 'uppercase',
      padding: '3px 8px',
      border: `1px solid ${highlight ? 'var(--rbx-accent-dim)' : 'var(--rbx-line)'}`,
      color: highlight ? 'var(--rbx-accent)' : 'var(--rbx-fg-muted)',
    }}>{phase}</span>
  );
};

// ——— Input ———
const Field = ({ label, value, onChange, type = 'text', placeholder, style }) => (
  <label style={{ display: 'flex', flexDirection: 'column', gap: 6, ...style }}>
    <span style={{
      fontFamily: 'var(--rbx-font-mono)', fontSize: 10, textTransform: 'uppercase',
      letterSpacing: '0.14em', color: 'var(--rbx-fg-dim)',
    }}>{label}</span>
    <input
      type={type} value={value || ''} placeholder={placeholder}
      onChange={e => onChange && onChange(e.target.value)}
      style={{
        background: 'var(--rbx-surface-1)', border: '1px solid var(--rbx-line)',
        color: 'var(--rbx-fg)', padding: '10px 12px',
        fontFamily: 'var(--rbx-font-sans)', fontSize: 13,
        borderRadius: 4, outline: 'none',
      }}
      onFocus={e => e.target.style.borderColor = 'var(--rbx-line-strong)'}
      onBlur={e => e.target.style.borderColor = 'var(--rbx-line)'}
    />
  </label>
);

// ——— Card ———
const Card = ({ children, style, padding = 22 }) => (
  <div style={{
    border: '1px solid var(--rbx-line)',
    background: 'var(--rbx-surface-1)',
    borderRadius: 8, padding,
    ...style,
  }}>{children}</div>
);

// ——— RBX mark (imported from logo) ———
const RbxMark = ({ size = 28 }) => (
  <img src="../../assets/bitmap.svg" alt="RBX" style={{ width: size, height: size, display: 'block' }} />
);

const RbxLogo = ({ subtitle = 'Systems', markSize = 36 }) => (
  <div style={{ display: 'flex', alignItems: 'center', gap: 12 }}>
    <RbxMark size={markSize} />
    <div style={{ display: 'flex', flexDirection: 'column', gap: 2, lineHeight: 1 }}>
      <span style={{ fontSize: 16, fontWeight: 600, letterSpacing: '-0.01em', color: 'var(--rbx-fg)' }}>RBX</span>
      <span style={{
        fontFamily: 'var(--rbx-font-mono)', fontSize: 9, textTransform: 'uppercase',
        letterSpacing: '0.14em', color: 'var(--rbx-fg-dim)',
      }}>{subtitle}</span>
    </div>
  </div>
);

// ——— Icons (inline line strokes, 1.5) ———
const Icon = ({ name, size = 16, color }) => {
  const paths = {
    arrow: <path d="M5 12h14M13 6l6 6-6 6" />,
    check: <path d="M20 6 9 17l-5-5" />,
    close: <path d="M18 6 6 18M6 6l12 12" />,
    menu: <><path d="M3 6h18M3 12h18M3 18h18" /></>,
    search: <><circle cx="11" cy="11" r="7" /><path d="m20 20-3.5-3.5" /></>,
    globe: <><circle cx="12" cy="12" r="9" /><path d="M3 12h18M12 3a14 14 0 010 18M12 3a14 14 0 000 18" /></>,
    activity: <path d="M3 12h4l3-9 4 18 3-9h4" />,
    server: <><rect x="3" y="4" width="18" height="7" rx="1" /><rect x="3" y="13" width="18" height="7" rx="1" /><path d="M7 7.5h.01M7 16.5h.01" /></>,
    git: <><circle cx="7" cy="7" r="2.5"/><circle cx="17" cy="17" r="2.5"/><circle cx="7" cy="17" r="2.5"/><path d="M7 9.5v5M9.5 7h5a3 3 0 013 3v4.5"/></>,
    chevron: <path d="m6 9 6 6 6-6" />,
    plus: <path d="M12 5v14M5 12h14" />,
    linkedin: <><rect x="3" y="3" width="18" height="18" rx="2"/><path d="M8 10v7M8 7v.01M12 17v-4a2 2 0 114 0v4M12 10v7"/></>,
    github: <path d="M12 2a10 10 0 00-3.16 19.49c.5.09.68-.22.68-.48v-1.7c-2.78.6-3.37-1.34-3.37-1.34-.45-1.15-1.1-1.46-1.1-1.46-.9-.62.07-.6.07-.6 1 .07 1.53 1.03 1.53 1.03.9 1.53 2.35 1.09 2.92.83.09-.65.35-1.09.63-1.34-2.22-.25-4.55-1.11-4.55-4.94 0-1.1.39-2 1.03-2.7-.1-.25-.45-1.28.1-2.67 0 0 .84-.27 2.75 1.03a9.54 9.54 0 015 0c1.9-1.3 2.75-1.03 2.75-1.03.55 1.4.2 2.42.1 2.68.64.69 1.03 1.58 1.03 2.7 0 3.83-2.34 4.68-4.57 4.93.36.31.68.92.68 1.85v2.74c0 .27.18.58.68.48A10 10 0 0012 2z" fill={color || 'currentColor'} stroke="none" />,
  };
  return (
    <svg viewBox="0 0 24 24" width={size} height={size}
      fill="none" stroke={color || 'currentColor'} strokeWidth="1.5"
      strokeLinecap="round" strokeLinejoin="round">
      {paths[name]}
    </svg>
  );
};

Object.assign(window, {
  Container, Hair, Eyebrow, Button, StatusDot, StatusPill, PhaseTag,
  Field, Card, RbxMark, RbxLogo, Icon,
});
