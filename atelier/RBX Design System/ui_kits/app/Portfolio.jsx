function PortfolioCard() {
  return (
    <Card>
      <Eyebrow>Portfolio · BTC</Eyebrow>
      <div className="rbx-metric">0.04823</div>
      <div className="rbx-delta rbx-delta--pos">+0.00142 · +3.03%</div>
      <div className="rbx-card__foot">Since inception · synced 2m ago</div>
    </Card>
  );
}

function EquityCard() {
  return (
    <Card>
      <Eyebrow>Equity · USDT</Eyebrow>
      <div className="rbx-metric">3,249.80</div>
      <div className="rbx-delta rbx-delta--pos">+98.40 · +3.12%</div>
      <div className="rbx-card__foot">Available margin · 1,842.00</div>
    </Card>
  );
}

function ExposureCard() {
  return (
    <Card>
      <Eyebrow>Liquidation distance</Eyebrow>
      <div className="rbx-metric rbx-metric--warn">12.40%</div>
      <div className="rbx-delta rbx-delta--neutral">Threshold · 10.00%</div>
      <div className="rbx-card__foot">2 open positions · 1 leveraged</div>
    </Card>
  );
}

function PositionsTable({ onSelect, selectedId }) {
  const rows = [
    { id: 'pos-01', sym: 'BTCUSDT', qty: '0.00482', entry: '66,820.00', mark: '67,420.50', pnl: '+2.89', pos: true, stop: '66,180.00' },
    { id: 'pos-02', sym: 'ETHUSDT', qty: '0.12400', entry: '3,280.40', mark: '3,241.10', pnl: '-4.87', pos: false, stop: '3,210.00' },
    { id: 'pos-03', sym: 'SOLUSDT', qty: '1.84500', entry: '148.20', mark: '152.95', pnl: '+8.76', pos: true, stop: '145.50' },
  ];
  return (
    <div className="rbx-panel">
      <div className="rbx-panel__head">
        <Eyebrow>Open positions · 3</Eyebrow>
        <Button variant="ghost" icon="refresh-cw">Refresh</Button>
      </div>
      <table className="rbx-table">
        <thead>
          <tr>
            <th>Symbol</th>
            <th className="num">Qty</th>
            <th className="num">Entry</th>
            <th className="num">Mark</th>
            <th className="num">Stop</th>
            <th className="num">P&amp;L</th>
            <th></th>
          </tr>
        </thead>
        <tbody>
          {rows.map(r => (
            <tr key={r.id} className={selectedId === r.id ? 'is-selected' : ''} onClick={() => onSelect?.(r.id)}>
              <td className="sym">{r.sym}</td>
              <td className="mono">{r.qty}</td>
              <td className="mono">{r.entry}</td>
              <td className="mono">{r.mark}</td>
              <td className="mono" style={{color:'var(--fg-2)'}}>{r.stop}</td>
              <td className={`mono ${r.pos ? 'pos' : 'neg'}`}>{r.pnl}</td>
              <td className="mono" style={{color:'var(--fg-2)'}}>→</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

Object.assign(window, { PortfolioCard, EquityCard, ExposureCard, PositionsTable });
