import React from 'react'
import Form from 'react-bootstrap/Form';

function CommandButton() {
  return(
    <Form>
      <div>
        <Form.Check // prettier-ignore
        type="switch"
        id="custom-switch"
        label="BTC"
      />
      </div>
    </Form>
  )
}

export default CommandButton