const { useState: useStateShell } = React;

function Sidebar({ route, setRoute }) {
  const items = [
    { id: 'dashboard', label: 'Dashboard', icon: 'layout-dashboard' },
    { id: 'positions', label: 'Positions', icon: 'layers' },
    { id: 'intent', label: 'Trading intents', icon: 'activity' },
    { id: 'patterns', label: 'Patterns', icon: 'bar-chart-3' },
    { id: 'audit', label: 'Audit trail', icon: 'clock' },
    { id: 'settings', label: 'Settings', icon: 'settings' },
  ];
  return (
    <aside className="rbx-sidebar">
      <div className="rbx-sidebar__brand">
        <div className="rbx-sidebar__bar" />
        <span>RBX</span>
      </div>
      <nav>
        {items.map((it) => (
          <button
            key={it.id}
            className={`rbx-nav-item ${route === it.id ? 'is-active' : ''}`}
            onClick={() => setRoute(it.id)}
          >
            <i data-lucide={it.icon}></i>
            <span>{it.label}</span>
          </button>
        ))}
      </nav>
      <div className="rbx-sidebar__foot">
        <div className="rbx-label">Runtime</div>
        <div className="rbx-sidebar__foot-row">
          <Badge tone="positive">Online</Badge>
          <span className="rbx-mono-sm">v0.104</span>
        </div>
        <div className="rbx-sidebar__foot-row">
          <Badge tone="warning">Dry-run</Badge>
        </div>
      </div>
    </aside>
  );
}

function Topbar({ title, eyebrow, actions }) {
  return (
    <div className="rbx-topbar">
      <div>
        <Eyebrow>{eyebrow}</Eyebrow>
        <h1 className="rbx-topbar__title">{title}</h1>
      </div>
      <div className="rbx-topbar__actions">{actions}</div>
    </div>
  );
}

Object.assign(window, { Sidebar, Topbar });
