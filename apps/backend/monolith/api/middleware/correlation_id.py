"""
Correlation ID middleware for request tracing.

Extracts or generates a unique request ID for each request,
propagates it through logs and returns it in response headers.
"""
import uuid
from django.utils.deprecation import MiddlewareMixin
from .logging_filter import set_correlation_id, clear_correlation_id


class CorrelationIDMiddleware(MiddlewareMixin):
    """
    Middleware to handle request correlation IDs.

    - Reads X-Request-ID from incoming request header
    - Generates new UUID if not present
    - Attaches to request object
    - Sets in thread-local for logging
    - Returns X-Request-ID in response header
    """

    def process_request(self, request):
        """Extract or generate correlation ID."""
        request_id = request.META.get('HTTP_X_REQUEST_ID')

        if not request_id:
            request_id = str(uuid.uuid4())

        request.correlation_id = request_id
        set_correlation_id(request_id)

    def process_response(self, request, response):
        """Add correlation ID to response headers and clear thread-local."""
        try:
            if hasattr(request, 'correlation_id'):
                response['X-Request-ID'] = request.correlation_id
        finally:
            # Always clear thread-local to prevent leakage between requests
            clear_correlation_id()

        return response

    def process_exception(self, request, exception):
        """Clear thread-local on exception to prevent leakage."""
        clear_correlation_id()
        return None  # Let Django handle the exception normally
