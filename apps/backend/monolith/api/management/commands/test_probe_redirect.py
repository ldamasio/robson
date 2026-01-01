"""
Management command to test probe endpoint SSL redirect behavior.

Usage:
    python manage.py test_probe_redirect

Expected output:
    /healthz: 200 (no redirect)
    /readyz: 200 (no redirect)
    /api/ping/: 301 (redirect to HTTPS) when accessed via HTTP
"""
from django.core.management.base import BaseCommand
from django.test import Client


class Command(BaseCommand):
    help = 'Test that probe endpoints do not redirect to HTTPS'

    def handle(self, *args, **options):
        client = Client()

        self.stdout.write(self.style.MIGRATE_HEADING('\n=== Testing Probe Endpoints ===\n'))

        # Test /healthz
        response = client.get('/healthz', secure=False)
        status = 'PASS' if response.status_code == 200 else 'FAIL'
        color = self.style.SUCCESS if status == 'PASS' else self.style.ERROR
        self.stdout.write(color(
            f'/healthz (HTTP): {response.status_code} - {status}'
        ))
        if response.status_code == 301:
            self.stdout.write(self.style.WARNING(
                f'  Redirect to: {response.get("Location", "N/A")}'
            ))

        # Test /readyz
        response = client.get('/readyz', secure=False)
        status = 'PASS' if response.status_code == 200 else 'FAIL'
        color = self.style.SUCCESS if status == 'PASS' else self.style.ERROR
        self.stdout.write(color(
            f'/readyz (HTTP): {response.status_code} - {status}'
        ))
        if response.status_code == 301:
            self.stdout.write(self.style.WARNING(
                f'  Redirect to: {response.get("Location", "N/A")}'
            ))

        # Test regular endpoint (should redirect)
        self.stdout.write(self.style.MIGRATE_HEADING('\n=== Testing Regular Endpoint (should redirect) ===\n'))
        response = client.get('/api/ping/', secure=False)
        status = 'PASS' if response.status_code == 301 else 'FAIL'
        color = self.style.SUCCESS if status == 'PASS' else self.style.ERROR
        self.stdout.write(color(
            f'/api/ping/ (HTTP): {response.status_code} - {status}'
        ))
        if response.status_code == 301:
            self.stdout.write(self.style.SUCCESS(
                f'  Redirect to: {response.get("Location", "N/A")}'
            ))

        self.stdout.write(self.style.MIGRATE_HEADING('\n=== Summary ==='))
        self.stdout.write('Probe endpoints should return 200 over HTTP (no redirect)')
        self.stdout.write('Regular endpoints should return 301 (redirect to HTTPS)')
