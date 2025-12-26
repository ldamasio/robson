import React, { useState, useEffect, useContext } from 'react'
import { Container, Tab, Tabs, Alert } from 'react-bootstrap'
import { useLocation } from 'react-router-dom'
import Header from '../components/common/Header'
import Footer from '../components/common/Footer'
import CommandButton from '../components/logged/CommandButton'
import StartNewOperation from '../components/logged/StartNewOperation'
import ManagePosition from '../components/logged/ManagePosition'
import ActualPrice from '../components/logged/ActualPrice'
import Trend from '../components/logged/Trend'
import Strategies from '../components/logged/Strategies'
import Patrimony from '../components/logged/Patrimony'
import BTCPortfolioDashboard from '../components/logged/BTCPortfolioDashboard'
import Balance from '../components/logged/Balance'
import Position from '../components/logged/Position'
import Risk from '../components/logged/Risk'
import Volume from '../components/logged/Volume'
import Chart from '../components/logged/Chart'
import Dataframe from '../components/logged/Dataframe'
import AuthContext from '../context/AuthContext'
import ErrorBoundary from '../components/common/ErrorBoundary'
import EmotionalGuard from '../components/logged/EmotionalGuard'
import MarginPositionCalculator from '../components/logged/MarginPositionCalculator'
import MarginPositions from '../components/logged/MarginPositions'
import PositionsDashboard from '../components/logged/PositionsDashboard'

const LoggedHomeScreen = () => {
  const location = useLocation()
  const { user } = useContext(AuthContext)
  
  // Check if we're in demo mode via query parameters
  const searchParams = new URLSearchParams(location.search)
  const isDemoMode = searchParams.get('demo') === 'true'
  const demoModeType = searchParams.get('mode')
  const demoApiKey = searchParams.get('apiKey')
  const demoSecretKey = searchParams.get('secretKey')
  
  // Demo user data for view-only mode
   const demoUser = {
     username: 'demo_user',
     email: 'demo@robsonbot.com',
     first_name: 'Demo',
     last_name: 'User'
   }
   
   // Get current user for display (real user or demo user)
   const currentUser = isDemoMode ? demoUser : user
   
   return (
    <div>
      <Header />
      <main className="py-5">
        <Container>
          {/* Demo Mode Alerts */}
          {isDemoMode && (
            <>
              {demoModeType === 'viewonly' && (
                <Alert variant="info" className="mb-4">
                  <strong>👁️ Modo Demo - Visualização Apenas</strong><br />
                  Você está no modo de demonstração. Todas as funcionalidades são exibidas 
                  mas nenhuma operação real será executada.
                </Alert>
              )}
              
              {demoModeType === 'testnet' && (
                <Alert variant="warning" className="mb-4">
                  <strong>🔑 Modo Demo - Testnet com Suas Chaves</strong><br />
                  Você está usando suas próprias chaves da Binance Testnet. 
                  Esta demo tem limite de 3 dias. Após esse período, considere assinar o plano Pro.
                </Alert>
              )}
            </>
          )}
          <Tabs defaultActiveKey="1">
            <Tab eventKey="1" title="Control Panel">
              <h1>Command Button</h1>
              <CommandButton />
              <h1>Start new operation</h1>
              <StartNewOperation />
              <h1>Manage position</h1>
              <ManagePosition />
              <h1>Actual Price</h1>
              <ErrorBoundary>
                <ActualPrice />
              </ErrorBoundary>
              <h1>Trend Now</h1>
              <Trend />
              <h1>Best Strategies</h1>
              <Strategies />
              <h1>Patrimony</h1>
              <Patrimony />
              <h1>Balance</h1>
              <Balance />
              <h1>Position</h1>
              <ErrorBoundary>
                <Position />
              </ErrorBoundary>
              <h1>Risk Indicator</h1>
              <Risk />
            </Tab>
            <Tab eventKey="2" title="Indicators">
              <h1>Volume BTC USDT Last 24h</h1>
              <Volume />
              <h1>BTC USDT 4 Hour Chart</h1>
              <ErrorBoundary>
                <Chart />
              </ErrorBoundary>
              <h1>BTC USDT Last Week Dataframe</h1>
              <Dataframe />
            </Tab>
            <Tab eventKey="3" title="🛡️ Emotional Guard">
              <div className="py-4">
                <ErrorBoundary>
                  <EmotionalGuard />
                </ErrorBoundary>
              </div>
            </Tab>
            <Tab eventKey="4" title="📊 Margin Trading">
              <div className="py-4">
                <ErrorBoundary>
                  <MarginPositionCalculator />
                </ErrorBoundary>
                <div className="mt-4">
                  <ErrorBoundary>
                    <MarginPositions />
                  </ErrorBoundary>
                </div>
              </div>
            </Tab>
            <Tab eventKey="5" title="💼 Portfolio">
              <div className="py-4">
                <ErrorBoundary>
                  <BTCPortfolioDashboard />
                </ErrorBoundary>
                <hr className="my-4" />
                <ErrorBoundary>
                  <PositionsDashboard />
                </ErrorBoundary>
              </div>
            </Tab>
          </Tabs>
        </Container>
      </main>
      <Footer />
    </div>
  );
}

export default LoggedHomeScreen;
