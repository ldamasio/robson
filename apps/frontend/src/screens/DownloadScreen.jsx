import React, { useState, useEffect } from 'react';
import { Container, Row, Col, Card, Button, Badge, Alert, Spinner } from 'react-bootstrap';
import { Download, Github, BookOpen, Code, Git, Calendar } from 'react-bootstrap-icons';
import Header from '../components/common/Header';
import Footer from '../components/common/Footer';

function DownloadScreen() {
  const [latestCommit, setLatestCommit] = useState(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState(null);

  const GITHUB_REPO = 'ldamasio/robson';
  const GITHUB_BRANCH = 'main';
  const DOWNLOAD_URL = `https://github.com/${GITHUB_REPO}/archive/refs/heads/${GITHUB_BRANCH}.zip`;

  useEffect(() => {
    // Fetch latest commit info from GitHub API
    fetch(`https://api.github.com/repos/${GITHUB_REPO}/commits/${GITHUB_BRANCH}`)
      .then(res => res.json())
      .then(data => {
        setLatestCommit(data);
        setLoading(false);
      })
      .catch(err => {
        console.error('Failed to fetch commit info:', err);
        setError('Could not fetch latest version info');
        setLoading(false);
      });
  }, []);

  const handleDownload = () => {
    window.location.href = DOWNLOAD_URL;
  };

  const formatDate = (dateString) => {
    if (!dateString) return 'Unknown';
    const date = new Date(dateString);
    return date.toLocaleDateString('en-US', {
      year: 'numeric',
      month: 'long',
      day: 'numeric',
    });
  };

  const getShortSha = (sha) => {
    return sha ? sha.substring(0, 7) : 'unknown';
  };

  return (
    <div>
      <Header />
      <main className="py-5" style={{ minHeight: '80vh', backgroundColor: '#f8f9fa' }}>
        <Container>
          {/* Hero Section */}
          <Row className="mb-5">
            <Col>
              <div className="text-center mb-4">
                <Code size={64} className="text-primary mb-3" />
                <h1 className="display-4 fw-bold">Download Robson Bot</h1>
                <p className="lead text-muted">
                  Open-source cryptocurrency trading platform
                </p>
              </div>
            </Col>
          </Row>

          {/* Main Download Card */}
          <Row className="justify-content-center mb-4">
            <Col md={8} lg={6}>
              <Card className="shadow-lg border-0">
                <Card.Body className="p-5">
                  <div className="text-center mb-4">
                    <h3 className="mb-3">
                      <Git className="me-2" />
                      Latest Version from <Badge bg="dark">{GITHUB_BRANCH}</Badge>
                    </h3>

                    {loading && (
                      <div className="my-4">
                        <Spinner animation="border" role="status" variant="primary">
                          <span className="visually-hidden">Loading...</span>
                        </Spinner>
                        <p className="text-muted mt-2">Fetching latest version...</p>
                      </div>
                    )}

                    {error && (
                      <Alert variant="warning" className="my-3">
                        {error}
                      </Alert>
                    )}

                    {!loading && latestCommit && (
                      <div className="my-4">
                        <div className="d-flex justify-content-between align-items-center mb-2 p-3 bg-light rounded">
                          <small className="text-muted">Commit:</small>
                          <code className="text-dark">
                            {getShortSha(latestCommit.sha)}
                          </code>
                        </div>
                        <div className="d-flex justify-content-between align-items-center mb-2 p-3 bg-light rounded">
                          <small className="text-muted">
                            <Calendar className="me-1" />
                            Updated:
                          </small>
                          <span className="text-dark">
                            {formatDate(latestCommit.commit?.committer?.date)}
                          </span>
                        </div>
                        <div className="p-3 bg-light rounded">
                          <small className="text-muted d-block mb-1">Message:</small>
                          <p className="mb-0 text-dark" style={{ fontSize: '0.9rem' }}>
                            {latestCommit.commit?.message?.split('\n')[0] || 'No message'}
                          </p>
                        </div>
                      </div>
                    )}
                  </div>

                  <div className="d-grid gap-2">
                    <Button
                      variant="primary"
                      size="lg"
                      onClick={handleDownload}
                      className="py-3"
                    >
                      <Download size={20} className="me-2" />
                      Download Source Code (.zip)
                    </Button>
                  </div>

                  <p className="text-center text-muted mt-3 mb-0" style={{ fontSize: '0.85rem' }}>
                    Downloads latest code from GitHub
                  </p>
                </Card.Body>
              </Card>
            </Col>
          </Row>

          {/* Quick Links */}
          <Row className="justify-content-center">
            <Col md={8} lg={6}>
              <Row className="g-3">
                <Col md={6}>
                  <Card className="h-100 border-0 shadow-sm hover-shadow">
                    <Card.Body className="text-center">
                      <Github size={32} className="text-dark mb-2" />
                      <Card.Title as="h5">Repository</Card.Title>
                      <Card.Text className="text-muted small">
                        View source code, issues, and contribute
                      </Card.Text>
                      <Button
                        variant="outline-dark"
                        size="sm"
                        href={`https://github.com/${GITHUB_REPO}`}
                        target="_blank"
                        rel="noopener noreferrer"
                      >
                        Visit GitHub
                      </Button>
                    </Card.Body>
                  </Card>
                </Col>
                <Col md={6}>
                  <Card className="h-100 border-0 shadow-sm hover-shadow">
                    <Card.Body className="text-center">
                      <BookOpen size={32} className="text-primary mb-2" />
                      <Card.Title as="h5">Documentation</Card.Title>
                      <Card.Text className="text-muted small">
                        Setup guides, API docs, and architecture
                      </Card.Text>
                      <Button
                        variant="outline-primary"
                        size="sm"
                        href={`https://github.com/${GITHUB_REPO}#readme`}
                        target="_blank"
                        rel="noopener noreferrer"
                      >
                        Read Docs
                      </Button>
                    </Card.Body>
                  </Card>
                </Col>
              </Row>
            </Col>
          </Row>

          {/* Features Info */}
          <Row className="justify-content-center mt-5">
            <Col md={8} lg={6}>
              <Card className="border-0 bg-transparent">
                <Card.Body>
                  <h5 className="mb-3 text-center">What's Included</h5>
                  <ul className="list-unstyled">
                    <li className="mb-2">
                      ✅ <strong>Backend:</strong> Django 5.2 + Python 3.12 (Hexagonal Architecture)
                    </li>
                    <li className="mb-2">
                      ✅ <strong>Frontend:</strong> React 18 + Vite
                    </li>
                    <li className="mb-2">
                      ✅ <strong>Infrastructure:</strong> Kubernetes (k3s) + GitOps (ArgoCD)
                    </li>
                    <li className="mb-2">
                      ✅ <strong>Trading:</strong> Binance integration, risk management, automated stops
                    </li>
                    <li className="mb-2">
                      ✅ <strong>Documentation:</strong> Architecture decisions, setup guides, API specs
                    </li>
                  </ul>
                </Card.Body>
              </Card>
            </Col>
          </Row>
        </Container>
      </main>
      <Footer />

      <style jsx="true">{`
        .hover-shadow {
          transition: box-shadow 0.3s ease;
        }
        .hover-shadow:hover {
          box-shadow: 0 0.5rem 1rem rgba(0, 0, 0, 0.15) !important;
        }
      `}</style>
    </div>
  );
}

export default DownloadScreen;
