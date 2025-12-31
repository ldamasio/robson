/**
 * Pattern Detector Screen
 *
 * Main screen for the Opportunity Detector feature.
 * Displays pattern detection dashboard, recent alerts, and strategy configurations.
 */

import React, { useState, useEffect, useContext } from 'react';
import { Container, Tab, Tabs, Alert, Row, Col, Spinner } from 'react-bootstrap';
import { toast } from 'react-toastify';
import Header from '../../components/common/Header';
import Footer from '../../components/common/Footer';
import ErrorBoundary from '../../components/common/ErrorBoundary';
import AuthContext from '../../context/AuthContext';
import { PatternHttp } from '../../adapters/http/PatternHttp';

// Import components
import PatternDashboard from '../../components/logged/patterns/PatternDashboard';
import PatternConfigList from '../../components/logged/patterns/PatternConfigList';
import PatternConfigForm from '../../components/logged/patterns/PatternConfigForm';
import PatternAlertsList from '../../components/logged/patterns/PatternAlertsList';
import StrategyPanel from '../../components/logged/patterns/StrategyPanel';

const PatternDetectorScreen = () => {
  const { authTokens } = useContext(AuthContext);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState(null);

  // Initialize API client
  const patternApi = new PatternHttp({
    baseUrl: import.meta.env.VITE_API_BASE_URL,
    getAuthToken: () => authTokens?.access,
  });

  // Data state
  const [dashboardData, setDashboardData] = useState(null);
  const [configs, setConfigs] = useState([]);
  const [catalog, setCatalog] = useState([]);
  const [recentAlerts, setRecentAlerts] = useState([]);

  // UI state
  const [activeTab, setActiveTab] = useState('dashboard');
  const [showConfigForm, setShowConfigForm] = useState(false);
  const [editingConfig, setEditingConfig] = useState(null);
  const [selectedStrategy, setSelectedStrategy] = useState(null);
  const [refreshKey, setRefreshKey] = useState(0);

  // Fetch dashboard data
  const fetchDashboard = async () => {
    try {
      const data = await patternApi.getDashboard();
      setDashboardData(data);
    } catch (err) {
      console.error('Failed to fetch dashboard:', err);
    }
  };

  // Fetch configurations
  const fetchConfigs = async () => {
    try {
      const data = await patternApi.getConfigs();
      setConfigs(data.results || []);
    } catch (err) {
      console.error('Failed to fetch configs:', err);
    }
  };

  // Fetch pattern catalog
  const fetchCatalog = async () => {
    try {
      const data = await patternApi.getCatalog();
      setCatalog(data.results || []);
    } catch (err) {
      console.error('Failed to fetch catalog:', err);
    }
  };

  // Fetch recent alerts
  const fetchRecentAlerts = async () => {
    try {
      const data = await patternApi.getRecentConfirms({ hours: 6 });
      setRecentAlerts(data.results || []);
    } catch (err) {
      console.error('Failed to fetch recent alerts:', err);
    }
  };

  // Initial data load
  useEffect(() => {
    const loadInitialData = async () => {
      setLoading(true);
      setError(null);
      try {
        await Promise.all([
          fetchDashboard(),
          fetchConfigs(),
          fetchCatalog(),
          fetchRecentAlerts(),
        ]);
      } catch (err) {
        setError(err.message);
        toast.error(`Failed to load pattern data: ${err.message}`);
      } finally {
        setLoading(false);
      }
    };

    loadInitialData();
  }, [refreshKey]);

  // Handle create config
  const handleCreateConfig = async (configData) => {
    try {
      await patternApi.createConfig(configData);
      toast.success('Configuration created successfully');
      setShowConfigForm(false);
      await fetchConfigs();
      setRefreshKey(prev => prev + 1);
    } catch (err) {
      toast.error(`Failed to create configuration: ${err.message}`);
      throw err;
    }
  };

  // Handle update config
  const handleUpdateConfig = async (configId, configData) => {
    try {
      await patternApi.updateConfig(configId, configData);
      toast.success('Configuration updated successfully');
      setEditingConfig(null);
      await fetchConfigs();
      setRefreshKey(prev => prev + 1);
    } catch (err) {
      toast.error(`Failed to update configuration: ${err.message}`);
      throw err;
    }
  };

  // Handle delete config
  const handleDeleteConfig = async (configId) => {
    try {
      await patternApi.deleteConfig(configId);
      toast.success('Configuration deleted successfully');
      await fetchConfigs();
      setRefreshKey(prev => prev + 1);
    } catch (err) {
      toast.error(`Failed to delete configuration: ${err.message}`);
    }
  };

  // Handle edit config
  const handleEditConfig = (config) => {
    setEditingConfig(config);
    setShowConfigForm(true);
  };

  // Handle scan trigger
  const handleScan = async (scanOptions) => {
    try {
      const result = await patternApi.triggerScan(scanOptions);
      toast.success(`Scan complete: ${result.summary.total_patterns} patterns detected`);
      setRefreshKey(prev => prev + 1);
      await fetchDashboard();
      await fetchRecentAlerts();
      return result;
    } catch (err) {
      toast.error(`Scan failed: ${err.message}`);
      throw err;
    }
  };

  // Handle pattern to plan
  const handlePatternToPlan = async (patternInstanceId) => {
    try {
      const result = await patternApi.patternToPlan({ patternInstanceId });
      if (result.success) {
        toast.success(`Plan created for ${result.plans_created} strategy/ies`);
      } else {
        toast.warn(`No plans created: ${result.errors?.join(', ') || 'No matching strategies'}`);
      }
      setRefreshKey(prev => prev + 1);
      return result;
    } catch (err) {
      toast.error(`Failed to create plan: ${err.message}`);
      throw err;
    }
  };

  // Handle select strategy for panel view
  const handleSelectStrategy = (strategyId) => {
    setSelectedStrategy(strategyId);
    setActiveTab('strategy');
  };

  // Loading state
  if (loading) {
    return (
      <div>
        <Header />
        <main className="py-5">
          <Container className="text-center">
            <Spinner animation="border" variant="primary" />
            <p className="mt-3 text-muted">Loading Pattern Detector...</p>
          </Container>
        </main>
        <Footer />
      </div>
    );
  }

  // Error state
  if (error && !dashboardData) {
    return (
      <div>
        <Header />
        <main className="py-5">
          <Container>
            <Alert variant="danger">
              <Alert.Heading>Error Loading Pattern Detector</Alert.Heading>
              <p>{error}</p>
            </Alert>
          </Container>
        </main>
        <Footer />
      </div>
    );
  }

  return (
    <div>
      <Header />
      <main className="py-5">
        <Container>
          {/* Header */}
          <div className="mb-4">
            <h1 className="mb-2">🎯 Opportunity Detector</h1>
            <p className="text-muted">
              Configure pattern detection strategies and monitor trading opportunities.
            </p>
          </div>

          {/* Important Notice */}
          <Alert variant="info" className="mb-4">
            <strong>ℹ️ How Pattern Detection Works</strong>
            <ul className="mb-0 mt-2">
              <li>Configure which patterns belong to which strategies</li>
              <li>Run scans to detect patterns across symbols and timeframes</li>
              <li>Send confirmed patterns to Robson's execution pipeline</li>
              <li><strong>This does NOT place orders directly.</strong> Plans go through the standard PLAN → VALIDATE → EXECUTE flow.</li>
            </ul>
          </Alert>

          {/* Tabs */}
          <Tabs activeKey={activeTab} onSelect={(k) => setActiveTab(k)} className="mb-4">
            <Tab eventKey="dashboard" title="📊 Dashboard">
              <div className="py-3">
                <ErrorBoundary>
                  <PatternDashboard
                    data={dashboardData}
                    recentAlerts={recentAlerts}
                    onRefresh={() => {
                      fetchDashboard();
                      fetchRecentAlerts();
                    }}
                  />
                </ErrorBoundary>
              </div>
            </Tab>

            <Tab eventKey="configs" title="⚙️ Strategy Configuration">
              <div className="py-3">
                <ErrorBoundary>
                  {showConfigForm ? (
                    <PatternConfigForm
                      catalog={catalog}
                      editConfig={editingConfig}
                      onSave={editingConfig ? handleUpdateConfig : handleCreateConfig}
                      onCancel={() => {
                        setShowConfigForm(false);
                        setEditingConfig(null);
                      }}
                    />
                  ) : (
                    <PatternConfigList
                      configs={configs}
                      catalog={catalog}
                      onEdit={handleEditConfig}
                      onDelete={handleDeleteConfig}
                      onCreateNew={() => setShowConfigForm(true)}
                      onSelectStrategy={handleSelectStrategy}
                      onRefresh={fetchConfigs}
                    />
                  )}
                </ErrorBoundary>
              </div>
            </Tab>

            <Tab eventKey="alerts" title="🔔 Recent Alerts">
              <div className="py-3">
                <ErrorBoundary>
                  <PatternAlertsList
                    alerts={recentAlerts}
                    onRefresh={fetchRecentAlerts}
                    onPatternToPlan={handlePatternToPlan}
                  />
                </ErrorBoundary>
              </div>
            </Tab>

            <Tab eventKey="strategy" title="🎯 Active Strategy Panel">
              <div className="py-3">
                <ErrorBoundary>
                  <StrategyPanel
                    configs={configs}
                    selectedStrategy={selectedStrategy}
                    onSelectStrategy={setSelectedStrategy}
                    onScan={handleScan}
                    onPatternToPlan={handlePatternToPlan}
                    onRefresh={() => setRefreshKey(prev => prev + 1)}
                  />
                </ErrorBoundary>
              </div>
            </Tab>
          </Tabs>
        </Container>
      </main>
      <Footer />
    </div>
  );
};

export default PatternDetectorScreen;
