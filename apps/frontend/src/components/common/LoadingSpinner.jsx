import React from 'react'

function LoadingSpinner({ label = 'Loading...' }) {
  return (
    <div className="d-flex align-items-center gap-2">
      <div className="spinner-border text-primary" role="status" aria-label={label}>
        <span className="visually-hidden">{label}</span>
      </div>
      <span>{label}</span>
    </div>
  )
}

export default LoadingSpinner
