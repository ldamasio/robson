"""
Logging filter to inject correlation ID into log records.
"""
import logging
from threading import local

_thread_locals = local()


def set_correlation_id(correlation_id):
    """Store correlation ID in thread-local storage."""
    _thread_locals.correlation_id = correlation_id


def get_correlation_id():
    """Retrieve correlation ID from thread-local storage."""
    return getattr(_thread_locals, 'correlation_id', None)


class CorrelationIDFilter(logging.Filter):
    """
    Logging filter that adds correlation_id to log records.

    Usage in LOGGING config:
        'filters': {
            'correlation_id': {
                '()': 'api.middleware.logging_filter.CorrelationIDFilter',
            },
        },
    """

    def filter(self, record):
        record.correlation_id = get_correlation_id() or 'no-request-id'
        return True
