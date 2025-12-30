import React, { useState } from "react";
import {
  Container,
  Row,
  Col,
  Card,
  Button,
  Form,
  Alert,
} from "react-bootstrap";
import { LinkContainer } from "react-router-bootstrap";
import Header from "../components/common/Header";
import Footer from "../components/common/Footer";

function DemoScreen() {
  const [demoMode, setDemoMode] = useState(null);
  const [apiKey, setApiKey] = useState("");
  const [secretKey, setSecretKey] = useState("");
  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState("");

  const handleViewOnlyDemo = () => {
    setIsLoading(true);
    setError("");

    setTimeout(() => {
      setIsLoading(false);
      window.location.href = "/dashboard?demo=true&mode=viewonly";
    }, 1000);
  };

  const handleJoinWaitlist = async () => {
    setIsLoading(true);
    setError("");

    try {
      const email = prompt("Enter your email to join the Pro plan waitlist:");

      if (!email) {
        setIsLoading(false);
        return;
      }

      if (!email.includes("@")) {
        throw new Error("Please enter a valid email");
      }

      const response = await fetch("/api/waitlist/join/", {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
        },
        body: JSON.stringify({
          email: email,
          is_demo_user: true,
        }),
      });

      if (!response.ok) {
        const errorData = await response.json();
        throw new Error(errorData.error || "Error joining waitlist");
      }

      const result = await response.json();
      alert(`✅ ${result.message}`);
    } catch (error) {
      setError(error.message);
    } finally {
      setIsLoading(false);
    }
  };

  const handleTestnetDemo = async (e) => {
    e.preventDefault();

    if (!apiKey || !secretKey) {
      setError("Please fill in both API keys");
      return;
    }

    setIsLoading(true);
    setError("");

    try {
      // Validate credentials first
      const validateResponse = await fetch("/api/demo/validate-credentials/", {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
        },
        body: JSON.stringify({
          api_key: apiKey,
          secret_key: secretKey,
        }),
      });

      if (!validateResponse.ok) {
        const errorData = await validateResponse.json();
        throw new Error(errorData.error || "Invalid credentials");
      }

      // Create demo account
      const demoResponse = await fetch("/api/demo/create/", {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
        },
        body: JSON.stringify({
          username: `demo_${Date.now()}`,
          email: `demo_${Date.now()}@robsonbot.com`,
          password: "demo_password_123",
          api_key: apiKey,
          secret_key: secretKey,
        }),
      });

      if (!demoResponse.ok) {
        const errorData = await demoResponse.json();
        throw new Error(errorData.error || "Error creating demo account");
      }

      const demoData = await demoResponse.json();

      // Store tokens and redirect
      localStorage.setItem("accessToken", demoData.tokens.access);
      localStorage.setItem("refreshToken", demoData.tokens.refresh);

      window.location.href = "/dashboard";
    } catch (error) {
      setIsLoading(false);
      setError(error.message || "Error creating demo account");
    }
  };

  return (
    <div className="d-flex flex-column min-vh-100">
      <Header />
      <main className="flex-grow-1 py-5">
        <Container>
          <Row className="justify-content-center">
            <Col md={8} lg={6}>
              <div className="text-center mb-5">
                <h1 className="fw-bold text-primary mb-3">Smart Demo</h1>
                <p className="text-secondary lead">
                  Choose how you want to experience Robson
                </p>
              </div>

              {!demoMode ? (
                <Row className="g-4">
                  <Col md={6}>
                    <Card className="h-100 border-0 shadow-sm demo-card">
                      <Card.Body className="text-center p-4">
                        <div className="mb-4">
                          <div
                            className="bg-primary bg-opacity-10 rounded-circle d-inline-flex align-items-center justify-content-center"
                            style={{ width: "80px", height: "80px" }}
                          >
                            <span className="fs-2 text-primary">👁️</span>
                          </div>
                        </div>
                        <h4 className="fw-bold mb-3">View Mode</h4>
                        <p className="text-secondary mb-4">
                          Just observe Robson operating with a managed demo
                          account. No risks, no fund movements.
                        </p>
                        <Button
                          variant="primary"
                          size="lg"
                          onClick={() => setDemoMode("viewonly")}
                          className="w-100"
                        >
                          Start Demo
                        </Button>
                      </Card.Body>
                    </Card>
                  </Col>

                  <Col md={6}>
                    <Card className="h-100 border-0 shadow-sm demo-card">
                      <Card.Body className="text-center p-4">
                        <div className="mb-4">
                          <div
                            className="bg-success bg-opacity-10 rounded-circle d-inline-flex align-items-center justify-content-center"
                            style={{ width: "80px", height: "80px" }}
                          >
                            <span className="fs-2 text-success">🔑</span>
                          </div>
                        </div>
                        <h4 className="fw-bold mb-3">Testnet with Your Keys</h4>
                        <p className="text-secondary mb-4">
                          Use your own Binance testnet keys to test with virtual
                          funds. 3-day limit.
                        </p>
                        <Button
                          variant="success"
                          size="lg"
                          onClick={() => setDemoMode("testnet")}
                          className="w-100"
                        >
                          Use My Keys
                        </Button>
                      </Card.Body>
                    </Card>
                  </Col>
                </Row>
              ) : (
                <Card className="border-0 shadow-sm">
                  <Card.Body className="p-5">
                    {demoMode === "viewonly" ? (
                      <div className="text-center">
                        <div className="mb-4">
                          <div
                            className="bg-primary bg-opacity-10 rounded-circle d-inline-flex align-items-center justify-content-center mx-auto"
                            style={{ width: "100px", height: "100px" }}
                          >
                            <span className="fs-1 text-primary">👁️</span>
                          </div>
                        </div>
                        <h3 className="fw-bold mb-3">View-Only Demo</h3>
                        <p className="text-secondary mb-4">
                          You will enter the dashboard in view mode where you
                          can observe Robson operating with a managed demo
                          account.
                        </p>

                        <div className="alert alert-info mb-4">
                          <strong>⚠️ Read-Only Mode</strong>
                          <br />
                          No real operations will be executed. View only.
                        </div>

                        <div className="d-grid gap-2 d-md-flex justify-content-md-center">
                          <Button
                            variant="outline-secondary"
                            onClick={() => setDemoMode(null)}
                            disabled={isLoading}
                          >
                            Back
                          </Button>
                          <Button
                            variant="primary"
                            onClick={handleViewOnlyDemo}
                            disabled={isLoading}
                          >
                            {isLoading ? "Loading..." : "Start Demo"}
                          </Button>
                        </div>

                        <div className="text-center mt-4 pt-3 border-top">
                          <p className="text-muted mb-2">Liked the demo?</p>
                          <Button
                            variant="outline-primary"
                            onClick={handleJoinWaitlist}
                            disabled={isLoading}
                            size="sm"
                          >
                            Join Pro Plan Waitlist
                          </Button>
                        </div>
                      </div>
                    ) : (
                      <div>
                        <div className="text-center mb-4">
                          <div
                            className="bg-success bg-opacity-10 rounded-circle d-inline-flex align-items-center justify-content-center mx-auto"
                            style={{ width: "100px", height: "100px" }}
                          >
                            <span className="fs-1 text-success">🔑</span>
                          </div>
                        </div>
                        <h3 className="fw-bold text-center mb-4">
                          Demo with Testnet
                        </h3>

                        {error && (
                          <Alert variant="danger" className="mb-4">
                            {error}
                          </Alert>
                        )}

                        <Form onSubmit={handleTestnetDemo}>
                          <Form.Group className="mb-3">
                            <Form.Label className="fw-bold">
                              Testnet API Key
                            </Form.Label>
                            <Form.Control
                              type="text"
                              placeholder="Your Binance Testnet API Key"
                              value={apiKey}
                              onChange={(e) => setApiKey(e.target.value)}
                              disabled={isLoading}
                              className="bg-light"
                            />
                            <Form.Text className="text-muted">
                              Available on your Binance Testnet dashboard
                            </Form.Text>
                          </Form.Group>

                          <Form.Group className="mb-4">
                            <Form.Label className="fw-bold">
                              Testnet Secret Key
                            </Form.Label>
                            <Form.Control
                              type="password"
                              placeholder="Your Binance Testnet Secret Key"
                              value={secretKey}
                              onChange={(e) => setSecretKey(e.target.value)}
                              disabled={isLoading}
                              className="bg-light"
                            />
                            <Form.Text className="text-muted">
                              Keep this key secret
                            </Form.Text>
                          </Form.Group>

                          <div className="alert alert-warning mb-4">
                            <strong>⚠️ Important</strong>
                            <br />
                            • Use only Binance Testnet keys
                            <br />
                            • Demo limited to 3 days
                            <br />• If you like it, subscribe to the Pro plan to
                            use production keys
                          </div>

                          <div className="d-grid gap-2 d-md-flex justify-content-md-center">
                            <Button
                              variant="outline-secondary"
                              onClick={() => setDemoMode(null)}
                              disabled={isLoading}
                            >
                              Back
                            </Button>
                            <Button
                              type="submit"
                              variant="success"
                              disabled={isLoading}
                            >
                              {isLoading
                                ? "Connecting..."
                                : "Start Demo with My Keys"}
                            </Button>
                          </div>
                        </Form>
                      </div>
                    )}
                  </Card.Body>
                </Card>
              )}

              <div className="text-center mt-5 pt-4">
                <p className="text-muted">
                  After the demo, if you want to continue using Robson with your
                  real account,
                  <LinkContainer to="/pricing">
                    <Button variant="link" className="p-0 ms-1">
                      subscribe to the Pro plan
                    </Button>
                  </LinkContainer>
                </p>
              </div>
            </Col>
          </Row>
        </Container>
      </main>
      <Footer />
    </div>
  );
}

export default DemoScreen;
