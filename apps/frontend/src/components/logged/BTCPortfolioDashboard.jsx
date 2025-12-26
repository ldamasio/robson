import React, { useState, useEffect } from 'react';
import {
  Card,
  Tab,
  Tabs,
  Row,
  Col,
  Badge,
  Spinner,
  Alert,
  Table,
  Button,
  Form,
  InputGroup,
} from 'react-bootstrap';
import axios from 'axios';
import {
  LineChart,
  Line,
  XAxis,
  YAxis,
  CartesianGrid,
  Tooltip,
  Legend,
  ResponsiveContainer,
  AreaChart,
  Area,
} from 'recharts';

/**
 * BTC Portfolio Dashboard - Complete Portfolio View
 *
 * Features:
 * - Tab-based navigation for clean UX
 * - Overview tab: Total value + profit metrics
 * - History tab: Interactive chart with timeline
 * - Transactions tab: Deposits/withdrawals with filtering
 * - Auto-refresh every 60 seconds
 * - BTC-denominated everything
 */
function BTCPortfolioDashboard() {
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState(null);
  const [activeTab, setActiveTab] = useState('overview');

  // Overview data
  const [portfolioData, setPortfolioData] = useState(null);
  const [profitData, setProfitData] = useState(null);

  // History data
  const [historyData, setHistoryData] = useState([]);
  const [historyLoading, setHistoryLoading] = useState(false);
  const [timeRange, setTimeRange] = useState('30'); // days

  // Transactions data
  const [transactions, setTransactions] = useState([]);
  const [transactionsFilter, setTransactionsFilter] = useState('all'); // all, deposit, withdrawal
  const [transactionsLoading, setTransactionsLoading] = useState(false);

  // Fetch overview data
  const fetchOverview = async () => {
    try {
      const BACKEND_URL = import.meta.env.VITE_API_BASE_URL;
      const [totalRes, profitRes] = await Promise.all([
        axios.get(`${BACKEND_URL}/api/portfolio/btc/total/`),
        axios.get(`${BACKEND_URL}/api/portfolio/btc/profit/`),
      ]);

      setPortfolioData(totalRes.data);
      setProfitData(profitRes.data);
    } catch (err) {
      console.error('Failed to fetch overview:', err);
      setError('Failed to load portfolio data');
    }
  };

  // Fetch history data
  const fetchHistory = async () => {
    setHistoryLoading(true);
    try {
      const BACKEND_URL = import.meta.env.VITE_API_BASE_URL;
      const { data } = await axios.get(
        `${BACKEND_URL}/api/portfolio/btc/history/?start_date=${getStartDate(
          timeRange
        )}`
      );

      // Transform data for chart
      const chartData = data.history.map((snapshot) => ({
        date: new Date(snapshot.snapshot_time).toLocaleDateString(),
        total: parseFloat(snapshot.total_btc),
        spot: parseFloat(snapshot.spot_btc),
        margin: parseFloat(snapshot.margin_btc),
      }));

      setHistoryData(chartData);
    } catch (err) {
      console.error('Failed to fetch history:', err);
    } finally {
      setHistoryLoading(false);
    }
  };

  // Fetch transactions
  const fetchTransactions = async () => {
    setTransactionsLoading(true);
    try {
      const BACKEND_URL = import.meta.env.VITE_API_BASE_URL;
      const filterParam =
        transactionsFilter !== 'all' ? `?type=${transactionsFilter}` : '';
      const { data } = await axios.get(
        `${BACKEND_URL}/api/portfolio/deposits-withdrawals/${filterParam}`
      );

      setTransactions(data.transactions);
    } catch (err) {
      console.error('Failed to fetch transactions:', err);
    } finally {
      setTransactionsLoading(false);
    }
  };

  // Helper to get start date
  const getStartDate = (days) => {
    const date = new Date();
    date.setDate(date.getDate() - parseInt(days));
    return date.toISOString().split('T')[0];
  };

  // Initial load
  useEffect(() => {
    const loadData = async () => {
      setLoading(true);
      await fetchOverview();
      setLoading(false);
    };

    loadData();
    const interval = setInterval(loadData, 60000);
    return () => clearInterval(interval);
  }, []);

  // Load history when tab switches to history
  useEffect(() => {
    if (activeTab === 'history' && historyData.length === 0) {
      fetchHistory();
    }
  }, [activeTab, timeRange]);

  // Load transactions when tab switches to transactions
  useEffect(() => {
    if (activeTab === 'transactions') {
      fetchTransactions();
    }
  }, [activeTab, transactionsFilter]);

  if (loading) {
    return (
      <Card>
        <Card.Body className="text-center">
          <Spinner animation="border" role="status">
            <span className="visually-hidden">Loading...</span>
          </Spinner>
          <p className="mt-2 text-muted">Loading portfolio data...</p>
        </Card.Body>
      </Card>
    );
  }

  if (error) {
    return (
      <Card className="border-danger">
        <Card.Body className="text-center">
          <Alert variant="danger">Error: {error}</Alert>
        </Card.Body>
      </Card>
    );
  }

  if (!portfolioData || !profitData) {
    return null;
  }

  // Parse values
  const totalBtc = parseFloat(portfolioData.total_btc);
  const profitBtc = parseFloat(profitData.profit_btc);
  const profitPercent = parseFloat(profitData.profit_percent);
  const isProfitable = profitBtc >= 0;
  const profitVariant = isProfitable ? 'success' : 'danger';
  const profitIcon = isProfitable ? '↑' : '↓';
  const profitSign = isProfitable ? '+' : '';

  const formatBTC = (value) => parseFloat(value).toFixed(8);

  return (
    <Card className="shadow-sm">
      <Card.Body>
        {/* Header */}
        <div className="d-flex justify-content-between align-items-center mb-4">
          <Card.Title className="mb-0">Portfolio Tracker (BTC)</Card.Title>
          <Badge bg="primary" className="ms-2">
            Live
          </Badge>
        </div>

        {/* Tabs for Navigation */}
        <Tabs
          activeKey={activeTab}
          onSelect={(k) => setActiveTab(k)}
          className="mb-4"
        >
          {/* Overview Tab */}
          <Tab eventKey="overview" title="📊 Overview">
            <Row className="mb-4">
              <Col>
                <div className="text-center p-4 bg-primary bg-opacity-10 rounded">
                  <small className="text-muted">Total Portfolio Value</small>
                  <h2 className="my-2">
                    <strong className="text-primary">{formatBTC(totalBtc)} BTC</strong>
                  </h2>
                </div>
              </Col>
            </Row>

            <Row className="mb-4">
              <Col className="text-center">
                <div className="p-4 bg-light rounded">
                  <small className="text-muted">
                    {isProfitable ? 'Profit' : 'Loss'} Since Inception
                  </small>
                  <h3 className={`my-2 text-${profitVariant}`}>
                    <Badge bg={profitVariant} className="p-2 fs-6">
                      {profitIcon} {profitSign}
                      {formatBTC(profitBtc)} BTC
                    </Badge>
                  </h3>
                  <small className={isProfitable ? 'text-success' : 'text-danger'}>
                    ({profitSign}
                    {profitPercent.toFixed(2)}%)
                  </small>
                </div>
              </Col>
            </Row>

            <Row>
              <Col md={6}>
                <div className="p-3 bg-light rounded h-100">
                  <h6 className="mb-3">Account Breakdown</h6>
                  <div className="d-flex justify-content-between mb-2">
                    <span>Spot:</span>
                    <strong>{formatBTC(portfolioData.spot_btc)} BTC</strong>
                  </div>
                  <div className="d-flex justify-content-between mb-2">
                    <span>Margin:</span>
                    <strong>{formatBTC(portfolioData.margin_btc)} BTC</strong>
                  </div>
                  <div className="d-flex justify-content-between">
                    <span>Margin Debt:</span>
                    <strong className="text-danger">
                      -{formatBTC(portfolioData.margin_debt_btc)} BTC
                    </strong>
                  </div>
                </div>
              </Col>

              <Col md={6}>
                <div className="p-3 bg-light rounded h-100">
                  <h6 className="mb-3">Transaction Summary</h6>
                  <div className="d-flex justify-content-between mb-2">
                    <span>Total Deposits:</span>
                    <strong className="text-success">
                      +{formatBTC(profitData.total_deposits_btc)} BTC
                    </strong>
                  </div>
                  <div className="d-flex justify-content-between mb-2">
                    <span>Total Withdrawals:</span>
                    <strong className="text-warning">
                      -{formatBTC(profitData.total_withdrawals_btc)} BTC
                    </strong>
                  </div>
                  <hr />
                  <div className="d-flex justify-content-between">
                    <span>Net Inflows:</span>
                    <strong>
                      {formatBTC(
                        profitData.total_deposits_btc -
                          profitData.total_withdrawals_btc
                      )}{' '}
                      BTC
                    </strong>
                  </div>
                </div>
              </Col>
            </Row>
          </Tab>

          {/* History Tab */}
          <Tab eventKey="history" title="📈 History">
            <div className="mb-4">
              <Form.Group>
                <Form.Label>Time Range</Form.Label>
                <Form.Select
                  value={timeRange}
                  onChange={(e) => setTimeRange(e.target.value)}
                >
                  <option value="7">Last 7 days</option>
                  <option value="30">Last 30 days</option>
                  <option value="90">Last 90 days</option>
                  <option value="365">Last year</option>
                </Form.Select>
              </Form.Group>
            </div>

            {historyLoading ? (
              <div className="text-center py-5">
                <Spinner animation="border" />
              </div>
            ) : historyData.length > 0 ? (
              <ResponsiveContainer width="100%" height={400}>
                <AreaChart data={historyData}>
                  <defs>
                    <linearGradient id="colorTotal" x1="0" y1="0" x2="0" y2="1">
                      <stop
                        offset="5%"
                        stopColor="#8884d8"
                        stopOpacity={0.8}
                      />
                      <stop
                        offset="95%"
                        stopColor="#8884d8"
                        stopOpacity={0}
                      />
                    </linearGradient>
                  </defs>
                  <CartesianGrid strokeDasharray="3 3" />
                  <XAxis
                    dataKey="date"
                    tick={{ fontSize: 12 }}
                  />
                  <YAxis
                    tick={{ fontSize: 12 }}
                    label={{ value: 'BTC', angle: -90, position: 'insideLeft' }}
                  />
                  <Tooltip
                    formatter={(value) => [`${parseFloat(value).toFixed(8)} BTC`, 'Total']}
                  />
                  <Legend />
                  <Area
                    type="monotone"
                    dataKey="total"
                    stroke="#8884d8"
                    fillOpacity={1}
                    fill="url(#colorTotal)"
                    name="Total (BTC)"
                  />
                </AreaChart>
              </ResponsiveContainer>
            ) : (
              <Alert variant="info">No historical data available</Alert>
            )}
          </Tab>

          {/* Transactions Tab */}
          <Tab eventKey="transactions" title="💰 Transactions">
            <div className="mb-4">
              <Form.Group>
                <Form.Label>Filter by Type</Form.Label>
                <Form.Select
                  value={transactionsFilter}
                  onChange={(e) => setTransactionsFilter(e.target.value)}
                >
                  <option value="all">All Transactions</option>
                  <option value="deposit">Deposits Only</option>
                  <option value="withdrawal">Withdrawals Only</option>
                </Form.Select>
              </Form.Group>
            </div>

            {transactionsLoading ? (
              <div className="text-center py-5">
                <Spinner animation="border" />
              </div>
            ) : transactions.length > 0 ? (
              <Table striped bordered hover responsive>
                <thead>
                  <tr>
                    <th>Date</th>
                    <th>Type</th>
                    <th>Asset</th>
                    <th>Amount</th>
                    <th>BTC Value</th>
                  </tr>
                </thead>
                <tbody>
                  {transactions.map((tx) => (
                    <tr key={tx.id}>
                      <td>
                        {tx.executed_at
                          ? new Date(tx.executed_at).toLocaleDateString()
                          : 'N/A'}
                      </td>
                      <td>
                        <Badge
                          bg={tx.type === 'DEPOSIT' ? 'success' : 'warning'}
                        >
                          {tx.type}
                        </Badge>
                      </td>
                      <td>{tx.asset}</td>
                      <td>{parseFloat(tx.quantity).toFixed(8)}</td>
                      <td>
                        <strong>{formatBTC(tx.btc_value)} BTC</strong>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </Table>
            ) : (
              <Alert variant="info">No transactions found</Alert>
            )}
          </Tab>
        </Tabs>

        {/* Footer */}
        <div className="text-center mt-4 pt-3 border-top">
          <small className="text-muted">
            Designed by RBX Robótica • Auto-refreshes every 60s
          </small>
        </div>
      </Card.Body>
    </Card>
  );
}

export default BTCPortfolioDashboard;
