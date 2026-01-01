"""
Django management command to reset admin user password.

Usage:
    python manage.py reset_admin_password --username admin --password newpassword
    python manage.py reset_admin_password --username admin  # Will prompt for password
"""

from django.core.management.base import BaseCommand, CommandError
from django.contrib.auth import get_user_model
from getpass import getpass


class Command(BaseCommand):
    help = 'Reset password for an admin user'

    def add_arguments(self, parser):
        parser.add_argument(
            '--username',
            type=str,
            required=True,
            help='Username of the user to reset password',
        )
        parser.add_argument(
            '--password',
            type=str,
            help='New password (if not provided, will prompt)',
        )
        parser.add_argument(
            '--email',
            type=str,
            help='Email address (only used if user does not exist)',
        )
        parser.add_argument(
            '--create-if-not-exists',
            action='store_true',
            help='Create user if it does not exist',
        )
        parser.add_argument(
            '--make-superuser',
            action='store_true',
            help='Make user a superuser (only if creating new user)',
        )

    def handle(self, *args, **options):
        username = options['username']
        password = options['password']
        email = options.get('email') or f'{username}@robsonbot.com'
        create_if_not_exists = options['create_if_not_exists']
        make_superuser = options['make_superuser']

        User = get_user_model()

        try:
            user = User.objects.get(username=username)
            self.stdout.write(self.style.SUCCESS(f'Found user: {username}'))
        except User.DoesNotExist:
            if create_if_not_exists:
                self.stdout.write(self.style.WARNING(f'User "{username}" not found. Creating...'))
                if make_superuser:
                    user = User.objects.create_superuser(username=username, email=email)
                else:
                    user = User.objects.create_user(username=username, email=email)
                self.stdout.write(self.style.SUCCESS(f'Created user: {username}'))
            else:
                raise CommandError(f'User "{username}" does not exist. Use --create-if-not-exists to create it.')

        # Get password if not provided
        if not password:
            password = getpass('Enter new password: ')
            password_confirm = getpass('Confirm new password: ')
            if password != password_confirm:
                raise CommandError('Passwords do not match!')

        # Validate password
        if not password or len(password) < 8:
            raise CommandError('Password must be at least 8 characters long')

        # Reset password
        user.set_password(password)
        user.is_active = True
        user.save()

        self.stdout.write(self.style.SUCCESS(f'Successfully reset password for user "{username}"'))
        self.stdout.write(self.style.SUCCESS(f'User is_active: {user.is_active}'))
        self.stdout.write(self.style.SUCCESS(f'User is_superuser: {user.is_superuser}'))
        self.stdout.write(self.style.SUCCESS(f'User is_staff: {user.is_staff}'))

