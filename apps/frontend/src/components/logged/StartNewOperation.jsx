import { useState } from 'react';
import { useNavigate } from 'react-router-dom';
import Button from 'react-bootstrap/Button';
import StartNewOperationModal from "./modals/StartNewOperationModal";
import { showSuccess } from '../../utils/notifications';

function StartNewOperation() {
  const navigate = useNavigate();
  const [modalShow, setModalShow] = useState(false);

  /**
   * Handle successful trading intent creation
   * @param {Object} intent - Created trading intent from API
   */
  const handleOperationCreated = (intent) => {
    console.log('Trading intent created:', intent);

    // Show success toast notification
    showSuccess('Trading plan created successfully!');

    // Navigate to status screen
    navigate(`/trading-intent/${intent.id}`);
  };

  return (
    <>
      <Button
        id="start-new-operation-btn"
        variant="primary"
        onClick={() => setModalShow(true)}
      >
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