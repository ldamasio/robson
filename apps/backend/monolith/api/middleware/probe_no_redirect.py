"""
Middleware to bypass SSL redirect for Kubernetes probe endpoints.

Problem: SECURE_SSL_REDIRECT=True causes 301 redirects on HTTP requests,
but kubelet probes hit pods directly over HTTP on port 8000 (no TLS).

Solution: Mark /healthz and /readyz requests as "secure" so SecurityMiddleware
skips the redirect. These are internal-only endpoints with no sensitive data.

CRITICAL: This middleware MUST be placed BEFORE SecurityMiddleware in settings.
"""


class ProbeNoRedirectMiddleware:
    """
    Prevent SSL redirect for Kubernetes probe endpoints.

    Forces /healthz and /readyz to be treated as secure (HTTPS) so that
    SecurityMiddleware doesn't redirect them. This allows kubelet to probe
    pods directly over HTTP without getting 301 responses.

    Must be placed BEFORE django.middleware.security.SecurityMiddleware.
    """

    def __init__(self, get_response):
        self.get_response = get_response

    def __call__(self, request):
        # Mark probe endpoints as secure to bypass SSL redirect
        if request.path in ('/healthz', '/readyz'):
            # Set the header that SECURE_PROXY_SSL_HEADER checks
            # This makes SecurityMiddleware think the request came via HTTPS
            request.META['HTTP_X_FORWARDED_PROTO'] = 'https'

        response = self.get_response(request)
        return response
