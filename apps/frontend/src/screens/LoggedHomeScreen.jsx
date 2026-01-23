import React, { useState, useEffect, useContext } from "react";
import { Container, Row, Col, Alert, Card } from "react-bootstrap";
import { useLocation } from "react-router-dom";
import Header from "../components/common/Header";
import Footer from "../components/common/Footer";
import StartNewOperation from "../components/logged/StartNewOperation";
import Balance from "../components/logged/Balance";
import Position from "../components/logged/Position";
import AuthContext from "../context/AuthContext";
import ErrorBoundary from "../components/common/ErrorBoundary";
const LoggedHomeScreen = () => {
  const location = useLocation();
  const { user } = useContext(AuthContext);

  // Check if we're in demo mode via query parameters
  const searchParams = new URLSearchParams(location.search);
  const isDemoMode = searchParams.get("demo") === "true";
  const demoModeType = searchParams.get("mode");

  // Demo user data for view-only mode
  const demoUser = {
    username: "demo_user",
    email: "demo@robsonbot.com",
    first_name: "Demo",
    last_name: "User",
  };

  // Get current user for display (real user or demo user)
  const currentUser = isDemoMode ? demoUser : user;

  return (
    <div>
      <Header />
      <main className="py-5">
        <Container>
          {/* Demo Mode Alerts */}
          {isDemoMode && (
            <>
              {demoModeType === "viewonly" && (
                <Alert variant="info" className="mb-4">
                  <strong>👁️ Demo Mode - View Only</strong>
                  <br />
                  You are in demo mode. All features are displayed but no real
                  operations will be executed.
                </Alert>
              )}

              {demoModeType === "testnet" && (
                <Alert variant="warning" className="mb-4">
                  <strong>🔑 Demo Mode - Testnet with Your Keys</strong>
                  <br />
                  You are using your own Binance Testnet keys. This demo has a
                  3-day limit. After that period, consider subscribing to the
                  Pro plan.
                </Alert>
              )}
            </>
          )}

          <h1 className="mb-4">Dashboard</h1>

          {/* Main Section: Create new operation */}
          <section className="mb-5">
            <Card>
              <Card.Body>
                <Card.Title as="h2">Nova Operação</Card.Title>
                <Card.Text className="text-muted mb-3">
                  Configure e inicie uma nova operação de trading
                </Card.Text>
                <StartNewOperation />
              </Card.Body>
            </Card>
          </section>

          {/* Section: Account Information */}
          <section className="mb-5">
            <h2 className="mb-3">Informações da Conta</h2>
            <Row>
              <Col md={6} className="mb-3">
                <ErrorBoundary>
                  <Balance />
                </ErrorBoundary>
              </Col>
              <Col md={6} className="mb-3">
                <ErrorBoundary>
                  <Position />
                </ErrorBoundary>
              </Col>
            </Row>
          </section>

          {/* TODO: Section for active operations list */}
          {/* This will be implemented when backend is ready */}
        </Container>
      </main>
      <Footer />
    </div>
  );
};
export default LoggedHomeScreen;
