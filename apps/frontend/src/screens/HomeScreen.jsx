import React, { useState, useEffect } from "react";
import {
  Container,
  Row,
  Col,
  Button,
  Card,
  Badge,
  Spinner,
} from "react-bootstrap";
import { LinkContainer } from "react-router-bootstrap";
import Header from "../components/common/Header";
import Footer from "../components/common/Footer";

const HomeScreen = () => {
  const [latestRelease, setLatestRelease] = useState(null);
  const [loadingRelease, setLoadingRelease] = useState(true);

  useEffect(() => {
    const fetchRelease = async () => {
      try {
        const response = await fetch(
          "https://api.github.com/repos/ldamasio/robson/releases/latest",
        );
        if (response.ok) {
          const data = await response.json();
          setLatestRelease(data);
        }
      } catch (error) {
        console.error("Failed to fetch latest release:", error);
      } finally {
        setLoadingRelease(false);
      }
    };

    fetchRelease();
  }, []);

  return (
    <div className="d-flex flex-column min-vh-100">
      <Header />
      <main>
        {/* Hero Section */}
        <section className="hero-section text-center">
          <Container>
            <div className="mb-4">
              {loadingRelease ? (
                <Spinner animation="border" variant="primary" size="sm" />
              ) : latestRelease ? (
                <a
                  href={latestRelease.html_url}
                  target="_blank"
                  rel="noopener noreferrer"
                  className="text-decoration-none"
                >
                  <Badge
                    bg="primary"
                    className="px-3 py-2 rounded-pill bg-opacity-25 text-primary border border-primary"
                  >
                    New Release: {latestRelease.name || latestRelease.tag_name}
                  </Badge>
                </a>
              ) : (
                <Badge
                  bg="secondary"
                  className="px-3 py-2 rounded-pill bg-opacity-25 text-secondary border border-secondary"
                >
                  v0.101 Available
                </Badge>
              )}
            </div>
            <h1 className="display-1 fw-bold mb-4">
              Smart Trading <br />
              <span className="text-gradient">Made Simple</span>
            </h1>
            <p
              className="lead text-secondary mb-5 mx-auto"
              style={{ maxWidth: "700px" }}
            >
              Robson Bot is an open-source algorithmic trading platform that
              empowers you with professional-grade risk management and automated
              position sizing.
            </p>
            <div className="d-flex gap-3 justify-content-center">
              <LinkContainer to="/signup">
                <Button
                  variant="primary"
                  size="lg"
                  className="rounded-pill px-5"
                >
                  Get Started
                </Button>
              </LinkContainer>
              <LinkContainer to="/login">
                <Button
                  variant="outline-light"
                  size="lg"
                  className="rounded-pill px-5"
                >
                  Login
                </Button>
              </LinkContainer>
            </div>
          </Container>
        </section>

        {/* Features Highlights */}
        <section className="section-padding bg-dark">
          <Container>
            <Row className="g-4">
              <Col md={4}>
                <Card className="h-100 card-premium p-4">
                  <Card.Body>
                    <div className="fs-1 mb-3">🛡️</div>
                    <Card.Title>Risk Management</Card.Title>
                    <Card.Text className="text-secondary">
                      Automated 1% risk rule calculation and dynamic position
                      sizing to protect your capital.
                    </Card.Text>
                  </Card.Body>
                </Card>
              </Col>
              <Col md={4}>
                <Card className="h-100 card-premium p-4">
                  <Card.Body>
                    <div className="fs-1 mb-3">⚡</div>
                    <Card.Title>Agentic Workflow</Card.Title>
                    <Card.Text className="text-secondary">
                      Plan, Validate, and Execute trades with a robust
                      safety-first workflow designed for precision.
                    </Card.Text>
                  </Card.Body>
                </Card>
              </Col>
              <Col md={4}>
                <Card className="h-100 card-premium p-4">
                  <Card.Body>
                    <div className="fs-1 mb-3">📱</div>
                    <Card.Title>Real-time Dashboard</Card.Title>
                    <Card.Text className="text-secondary">
                      Monitor assets, track performance, and manage signals from
                      a sleek, responsive interface.
                    </Card.Text>
                  </Card.Body>
                </Card>
              </Col>
            </Row>
          </Container>
        </section>

        {/* CTA Section */}
        <section className="section-padding text-center">
          <Container>
            <div className="p-5 rounded-3 bg-glass border border-secondary">
              <h2 className="mb-3">Ready to upgrade your trading?</h2>
              <p className="text-secondary mb-4">
                Join our community of traders and developers using Robson to
                automate their strategies.
              </p>
              <a
                href="https://github.com/ldamasio/robson"
                target="_blank"
                rel="noopener noreferrer"
              >
                <Button
                  variant="light"
                  size="lg"
                  className="rounded-pill fw-bold"
                >
                  View on GitHub
                </Button>
              </a>
            </div>
          </Container>
        </section>
      </main>
      <Footer />
    </div>
  );
};

export default HomeScreen;
