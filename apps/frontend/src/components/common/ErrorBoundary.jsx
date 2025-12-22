import React from 'react'
import { toast } from 'react-toastify'

class ErrorBoundary extends React.Component {
  constructor(props) {
    super(props)
    this.state = { hasError: false }
  }

  static getDerivedStateFromError() {
    return { hasError: true }
  }

  componentDidCatch(error, errorInfo) {
    console.error('UI error:', error, errorInfo)
    if (typeof this.props.onError === 'function') {
      this.props.onError(error)
    } else {
      toast.error('Something went wrong while rendering this section.')
    }
  }

  render() {
    if (this.state.hasError) {
      return this.props.fallback || (
        <div className="alert alert-danger" role="alert">
          Unable to load this section. Please refresh and try again.
        </div>
      )
    }

    return this.props.children
  }
}

export default ErrorBoundary
