"""
Signals for clients app.

Auto-assign default client to newly created users without a client.
This ensures all users have a tenant association for multi-tenancy.
"""

from django.db.models.signals import post_save
from django.dispatch import receiver
from django.core.exceptions import ObjectDoesNotExist


@receiver(post_save, sender='clients.CustomUser')
def assign_default_client_on_user_creation(sender, instance, created, **kwargs):
    """
    Automatically assign the first available Client to newly created users.

    This signal ensures that every user has a client association for
    multi-tenancy isolation. Users created without an explicit client
    will be assigned to the default Client.

    Args:
        sender: CustomUser model
        instance: The user instance being saved
        created: Boolean indicating if this is a new user
        **kwargs: Additional signal arguments
    """
    if not created:
        return

    # Only assign if user doesn't already have a client
    if instance.client_id is not None:
        return

    from clients.models import Client

    # Get or create default client
    default_client, _ = Client.objects.get_or_create(
        email="default@rbx.ia.br",
        defaults={
            "name": "Default Client",
            "is_active": True
        }
    )

    # Associate user with default client
    instance.client = default_client

    # Save without triggering signals again (using update_fields)
    # We need to save the full instance but be careful about recursion
    # Using queryset.update() would bypass signals but not update instance
    # So we save normally - the signal checks created=False so won't recurse
    instance.save(update_fields=['client_id'])
