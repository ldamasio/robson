const { useState } = React;

function Button({ variant = 'secondary', children, icon, onClick, disabled }) {
  return (
    <button className={`rbx-btn rbx-btn--${variant}`} onClick={onClick} disabled={disabled}>
      {icon && <i data-lucide={icon}></i>}
      <span>{children}</span>
    </button>
  );
}

function Badge({ tone = 'neutral', children, dot = true }) {
  return (
    <span className={`rbx-chip rbx-chip--${tone}`}>
      {dot && <span className="rbx-chip__dot" />}
      {children}
    </span>
  );
}

function Field({ label, value, mono, error, hint, suffix, onChange, readOnly, width }) {
  return (
    <div className="rbx-field" style={{ width }}>
      <label>{label}</label>
      <div className={`rbx-input ${error ? 'is-error' : ''}`}>
        <input
          value={value}
          onChange={onChange ? (e) => onChange(e.target.value) : undefined}
          readOnly={readOnly}
          className={mono ? 'mono' : ''}
        />
        {suffix && <span className="rbx-input__suffix">{suffix}</span>}
      </div>
      {hint && !error && <div className="rbx-field__hint">{hint}</div>}
      {error && <div className="rbx-field__err">{error}</div>}
    </div>
  );
}

function Eyebrow({ children }) {
  return <div className="rbx-eyebrow">{children}</div>;
}

function Card({ children, padding = 20, onClick, className = '' }) {
  return (
    <div className={`rbx-card ${className}`} style={{ padding }} onClick={onClick}>
      {children}
    </div>
  );
}

Object.assign(window, { Button, Badge, Field, Eyebrow, Card });
