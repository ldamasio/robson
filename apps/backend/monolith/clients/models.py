"""
Client and User models for multi-tenant trading platform.

Clients represent tenants (trading accounts) with their own API credentials.
CustomUser extends Django's user model with client association.

Credentials (access_key, secret_key) are stored encrypted using Fernet.
"""

from django.db import models
from django.contrib.auth.models import AbstractUser
from typing import Optional


class Client(models.Model):
    """
    Tenant model representing a trading account.
    
    Each client can have multiple users and their own exchange credentials.
    Credentials are stored encrypted in the database.
    
    Attributes:
        name: Display name for the client
        email: Unique email for the client account
        api_url: Optional custom API URL (for different exchanges)
        stream_url: Optional custom WebSocket URL
        access_key: Encrypted exchange API key
        secret_key: Encrypted exchange secret key
    """
    
    tenant_id = 'id'
    name = models.CharField(max_length=50)
    address = models.CharField(max_length=255, blank=True)
    email = models.CharField(max_length=50, unique=True)
    api_url = models.CharField(max_length=255, blank=True)
    stream_url = models.CharField(max_length=255, blank=True)
    
    # Encrypted credentials - stored as Fernet-encrypted base64 strings
    access_key = models.CharField(max_length=500, blank=True)
    secret_key = models.CharField(max_length=500, blank=True)
    
    # Metadata
    created_at = models.DateTimeField(auto_now_add=True)
    updated_at = models.DateTimeField(auto_now=True)
    
    # Trading preferences
    is_active = models.BooleanField(default=True)
    
    # Demo trial period
    demo_created_at = models.DateTimeField(blank=True, null=True)
    demo_expires_at = models.DateTimeField(blank=True, null=True)
    is_demo_account = models.BooleanField(default=False)
    
    class Meta:
        ordering = ['-created_at']
    
    def __str__(self):
        return self.name
    
    def set_credentials(self, api_key: str, secret_key: str) -> None:
        """
        Set encrypted API credentials.
        
        Args:
            api_key: Plain text API key
            secret_key: Plain text secret key
        """
        from .crypto import encrypt_credential
        self.access_key = encrypt_credential(api_key)
        self.secret_key = encrypt_credential(secret_key)
    
    def get_api_key(self) -> Optional[str]:
        """
        Get decrypted API key.
        
        Returns:
            Decrypted API key or None if not set/decryption fails
        """
        from .crypto import decrypt_credential
        return decrypt_credential(self.access_key)
    
    def get_secret_key(self) -> Optional[str]:
        """
        Get decrypted secret key.
        
        Returns:
            Decrypted secret key or None if not set/decryption fails
        """
        from .crypto import decrypt_credential
        return decrypt_credential(self.secret_key)
    
    def has_credentials(self) -> bool:
        """Check if client has configured credentials."""
        return bool(self.access_key and self.secret_key)
    
    @property
    def masked_api_key(self) -> str:
        """Get masked version of API key for display (first 4 + last 4 chars)."""
        key = self.get_api_key()
        if not key or len(key) < 12:
            return "****"
        return f"{key[:4]}...{key[-4:]}"
    
    def start_demo_trial(self, days: int = 3) -> None:
        """
        Start a demo trial period for this client.
        
        Args:
            days: Number of days for the trial (default: 3)
        """
        from django.utils.timezone import now
        from datetime import timedelta
        
        self.demo_created_at = now()
        self.demo_expires_at = now() + timedelta(days=days)
        self.is_demo_account = True
        self.save()
    
    def is_demo_expired(self) -> bool:
        """
        Check if the demo trial has expired.
        
        Returns:
            True if demo has expired, False otherwise
        """
        from django.utils.timezone import now
        
        if not self.is_demo_account or not self.demo_expires_at:
            return False
        
        return now() > self.demo_expires_at
    
    def get_demo_remaining_days(self) -> int:
        """
        Get remaining days in demo trial.
        
        Returns:
            Number of days remaining (0 if expired or not demo)
        """
        from django.utils.timezone import now
        
        if not self.is_demo_account or not self.demo_expires_at:
            return 0
        
        if self.is_demo_expired():
            return 0
        
        remaining = self.demo_expires_at - now()
        return max(0, remaining.days)
    
    def upgrade_to_pro(self) -> None:
        """
        Upgrade demo account to full Pro account.
        """
        self.is_demo_account = False
        self.demo_expires_at = None
        self.save()


class WaitlistEntry(models.Model):
    """
    Model for Pro plan waitlist entries.
    
    Users can join the waitlist to be notified when Pro plan
    becomes available with payment integration.
    """
    
    email = models.EmailField(unique=True)
    client = models.ForeignKey(
        Client, 
        blank=True, 
        null=True, 
        on_delete=models.SET_NULL,
        related_name='waitlist_entries'
    )
    is_demo_user = models.BooleanField(default=False)
    created_at = models.DateTimeField(auto_now_add=True)
    notified_at = models.DateTimeField(blank=True, null=True)
    
    class Meta:
        verbose_name_plural = "Waitlist entries"
        ordering = ['created_at']
    
    def __str__(self):
        return f"{self.email} ({'Demo' if self.is_demo_user else 'New'})"
    
    def mark_notified(self) -> None:
        """Mark this waitlist entry as notified."""
        from django.utils.timezone import now
        self.notified_at = now()
        self.save()


class CustomUser(AbstractUser):
    """
    Extended user model with client (tenant) association.
    
    Users belong to a Client and inherit the client's trading credentials.
    """
    
    client = models.ForeignKey(
        Client, 
        blank=True, 
        null=True, 
        on_delete=models.SET_NULL,
        related_name='users'
    )
    tenant_id = 'client_id'
    account = models.BooleanField(default=False)

    def __str__(self):
        return self.username
    
    @property
    def tenant(self) -> Optional[Client]:
        """Alias for client for clearer multi-tenant semantics."""
        return self.client
    
    def can_trade(self) -> bool:
        """Check if user can execute trades (has client with credentials)."""
        return self.client is not None and self.client.has_credentials()

