import { useState } from 'react';
import Button from 'react-bootstrap/Button';
import StartNewOperationModal from "./modals/StartNewOperationModal";

function StartNewOperation() {
  const [modalShow, setModalShow] = useState(false);

  return (
    <>
      <Button variant="primary" onClick={() => setModalShow(true)}>
        Start New Operation
      </Button>

      <StartNewOperationModal show={modalShow} onHide={() => setModalShow(false)} />
    </>
  );
}

export default StartNewOperation;