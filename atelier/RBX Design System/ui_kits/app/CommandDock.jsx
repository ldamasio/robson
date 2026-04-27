const { useState: useCmdState, useRef, useEffect } = React;

function CommandDock() {
  const [input, setInput] = useCmdState('');
  const [feed, setFeed] = useCmdState([
    { role: 'system', text: 'Connected · tenant acme-prod · dry-run default', ts: '09:14:22' },
    { role: 'user', text: 'plan buy BTCUSDT 0.001 --limit 67420', ts: '09:14:28' },
    { role: 'system', text: 'plan-2847 created · pending validation', ts: '09:14:28' },
  ]);

  const send = () => {
    if (!input.trim()) return;
    const ts = new Date().toTimeString().slice(0, 8);
    const next = [...feed, { role: 'user', text: input, ts }];
    next.push({ role: 'system', text: 'Command accepted · routing to validator', ts });
    setFeed(next);
    setInput('');
  };

  return (
    <div className="rbx-dock">
      <div className="rbx-dock__topline">
        <div className="rbx-dock__brand">
          <span className="rbx-dock__dot is-online" />
          <span>Robson · online</span>
          <span className="rbx-dock__sep">·</span>
          <span style={{color:'var(--fg-2)'}}>acme-prod</span>
        </div>
        <div className="rbx-dock__cmds">
          <span>/plan</span><span>/validate</span><span>/execute</span><span>/close</span>
        </div>
      </div>
      <div className="rbx-dock__context">
        <Badge tone="warning">Dry-run</Badge>
        <Badge tone="info">BTCUSDT · 67,420.50</Badge>
        <Badge tone="positive">Portfolio +3.03%</Badge>
        <Badge tone="neutral">Liq 12.40%</Badge>
      </div>
      <div className="rbx-dock__feed">
        {feed.map((m, i) => (
          <div key={i} className={`rbx-dock__msg is-${m.role}`}>
            <div className="rbx-dock__role">{m.role === 'user' ? 'operator' : 'robson'} · {m.ts}</div>
            <div className="rbx-dock__text">{m.text}</div>
          </div>
        ))}
      </div>
      <div className="rbx-dock__prompt">
        <span className="rbx-dock__caret">&gt;_</span>
        <input
          className="rbx-dock__input"
          value={input}
          onChange={(e) => setInput(e.target.value)}
          onKeyDown={(e) => e.key === 'Enter' && send()}
          placeholder="plan | validate | execute | close | status"
        />
        <button className="rbx-dock__send" onClick={send} disabled={!input.trim()}>Send</button>
      </div>
    </div>
  );
}

Object.assign(window, { CommandDock });
