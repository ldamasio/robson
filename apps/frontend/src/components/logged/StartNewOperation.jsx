import { useState } from 'react';
import Button from 'react-bootstrap/Button';
import Alert from 'react-bootstrap/Alert';
import StartNewOperationModal from "./modals/StartNewOperationModal";

function StartNewOperation() {
  const [modalShow, setModalShow] = useState(false);
  const [successMessage, setSuccessMessage] = useState(null);

  /**
   * Handle successful trading intent creation
   * @param {Object} intent - Created trading intent from API
   */
  const handleOperationCreated = (intent) => {
    console.log('Trading intent created:', intent);

    // Show success message
    setSuccessMessage(
      `Trading intent created successfully! Plan ID: ${intent.id} (${intent.side} ${intent.symbol_display})`
    );

    // Auto-hide success message after 5 seconds
    setTimeout(() => {
      setSuccessMessage(null);
    }, 5000);

    // Future enhancements:
    // - Navigate to intent status page
    // - Refresh operations list
    // - Show toast notification instead of inline alert
  };

  return (
    <>
      {successMessage && (
        <Alert variant="success" dismissible onClose={() => setSuccessMessage(null)} className="mb-3">
          {successMessage}
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