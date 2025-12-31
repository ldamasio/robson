"""
Health check endpoints with Kubernetes-compatible semantics.

Endpoints:
- /healthz - Liveness probe (fast, no dependency checks)
- /readyz - Readiness probe (checks DB, cache, critical dependencies)
"""
from django.http import JsonResponse
from django.views.decorators.csrf import csrf_exempt
from django.views.decorators.http import require_GET
from django.db import connection
from django.core.cache import cache
import logging

logger = logging.getLogger(__name__)


@require_GET
@csrf_exempt
def healthz(request):
    """
    Liveness probe endpoint.

    Returns 200 OK if the application process is alive.
    Does NOT check dependencies (DB, cache, etc).
    Kubernetes uses this to know if pod needs restart.

    Fast response required (< 100ms).
    """
    return JsonResponse({
        'status': 'alive',
        'service': 'robson-backend',
    }, status=200)


@require_GET
@csrf_exempt
def readyz(request):
    """
    Readiness probe endpoint.

    Returns 200 OK only if application can serve traffic.
    Checks critical dependencies: Database, Cache.
    Kubernetes uses this to add/remove pod from load balancer.

    Failure scenarios:
    - Database unreachable
    - Cache unavailable
    - Migration pending (optional)

    Response should be < 1 second.
    """
    checks = {}
    all_healthy = True

    # Check database connection
    try:
        with connection.cursor() as cursor:
            cursor.execute("SELECT 1")
            cursor.fetchone()
        checks['database'] = 'healthy'
    except Exception as e:
        logger.error(f"Database readiness check failed: {e}")
        checks['database'] = 'unhealthy'
        all_healthy = False

    # Check cache connection
    try:
        cache.set('readyz_check', '1', timeout=1)
        value = cache.get('readyz_check')
        if value == '1':
            checks['cache'] = 'healthy'
        else:
            raise ValueError("Cache returned unexpected value")
    except Exception as e:
        logger.error(f"Cache readiness check failed: {e}")
        checks['cache'] = 'unhealthy'
        all_healthy = False

    status_code = 200 if all_healthy else 503

    return JsonResponse({
        'status': 'ready' if all_healthy else 'not_ready',
        'service': 'robson-backend',
        'checks': checks,
    }, status=status_code)
