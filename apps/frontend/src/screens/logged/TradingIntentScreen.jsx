import React from 'react';
import { useParams, useNavigate } from 'react-router-dom';
import Container from 'react-bootstrap/Container';
import Button from 'react-bootstrap/Button';
import Header from '../../components/common/Header';
import Footer from '../../components/common/Footer';
import TradingIntentStatus from '../../components/logged/TradingIntentStatus';
import ErrorBoundary from '../../components/common/ErrorBoundary';

/**
 * TradingIntentScreen - Screen for viewing a single trading intent status.
 *
 * Displays the full status, validation results, and execution results
 * for a specific trading intent.
 */
function TradingIntentScreen() {
  const { intentId } = useParams();
  const navigate = useNavigate();

  // Handle back button
  const handleBack = () => {
    navigate('/dashboard');
  };

  return (
    <div>
      <Header />
      <main className="py-5">
        <Container>
          <div className="d-flex justify-content-between align-items-center mb-4">
            <h1>Trading Intent Status</h1>
            <Button variant="outline-secondary" onClick={handleBack}>
              ← Back to Dashboard
            </Button>
          </div>

          <ErrorBoundary>
            <TradingIntentStatus intentId={intentId} showDetails={true} />
          </ErrorBoundary>
        </Container>
      </main>
      <Footer />
    </div>
  );
}

export default TradingIntentScreen;
