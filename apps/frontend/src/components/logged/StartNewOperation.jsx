import { useState } from 'react';
import { useNavigate } from 'react-router-dom';
import Button from 'react-bootstrap/Button';
import Alert from 'react-bootstrap/Alert';
import StartNewOperationModal from "./modals/StartNewOperationModal";

function StartNewOperation() {
  const navigate = useNavigate();
  const [modalShow, setModalShow] = useState(false);
  const [createdIntentId, setCreatedIntentId] = useState(null);

  /**
   * Handle successful trading intent creation
   * @param {Object} intent - Created trading intent from API
   */
  const handleOperationCreated = (intent) => {
    console.log('Trading intent created:', intent);

    // Store intent ID for showing link
    setCreatedIntentId(intent.id);

    // Auto-hide after 10 seconds
    setTimeout(() => {
      setCreatedIntentId(null);
    }, 10000);
  };

  const handleViewStatus = () => {
    if (createdIntentId) {
      navigate(`/trading-intent/${createdIntentId}`);
    }
  };

  return (
    <>
      {createdIntentId && (
        <Alert variant="success" dismissible onClose={() => setCreatedIntentId(null)} className="mb-3">
          <Alert.Heading>Trading Intent Created!</Alert.Heading>
          <p className="mb-2">Your trading plan has been created successfully.</p>
          <hr />
          <div className="d-flex justify-content-end">
            <Button variant="outline-success" size="sm" onClick={handleViewStatus}>
              View Status →
            </Button>
          </div>
        </Alert>
      )}

      <Button variant="primary" onClick={() => setModalShow(true)}>
        Start New Operation
      </Button>

      <StartNewOperationModal
        show={modalShow}
        onHide={() => setModalShow(false)}
        onSuccess={handleOperationCreated}
      />
    </>
  );
}

export default StartNewOperation;