"""
Encryption utilities for storing sensitive client credentials.

Uses Fernet symmetric encryption from the cryptography library.
Credentials are encrypted before storage and decrypted on retrieval.

The encryption key is derived from:
1. RBS_CREDENTIAL_ENCRYPTION_KEY environment variable (preferred)
2. RBS_SECRET_KEY as fallback (Django's secret key)

IMPORTANT: If the encryption key changes, previously encrypted data
will become unreadable. Ensure key rotation is handled properly.
"""

import base64
import hashlib
import logging
from typing import Optional

from cryptography.fernet import Fernet, InvalidToken
from django.conf import settings

logger = logging.getLogger(__name__)


def _get_encryption_key() -> bytes:
    """
    Get or derive the encryption key for credentials.
    
    Uses CREDENTIAL_ENCRYPTION_KEY if set, otherwise derives from SECRET_KEY.
    
    Returns:
        32-byte key suitable for Fernet encryption
    """
    # Try explicit encryption key first
    key = getattr(settings, 'CREDENTIAL_ENCRYPTION_KEY', '')
    
    if not key:
        # Fall back to deriving from Django SECRET_KEY
        key = settings.SECRET_KEY
        logger.debug("Using derived encryption key from SECRET_KEY")
    
    # Ensure key is 32 bytes (Fernet requires URL-safe base64 of 32 bytes)
    # We use SHA-256 to derive a consistent 32-byte key
    key_bytes = hashlib.sha256(key.encode()).digest()
    return base64.urlsafe_b64encode(key_bytes)


def get_fernet() -> Fernet:
    """Get a Fernet instance for encryption/decryption."""
    return Fernet(_get_encryption_key())


def encrypt_credential(plaintext: str) -> str:
    """
    Encrypt a credential for storage.
    
    Args:
        plaintext: The credential to encrypt
        
    Returns:
        Base64-encoded encrypted string
    """
    if not plaintext:
        return ""
    
    fernet = get_fernet()
    encrypted = fernet.encrypt(plaintext.encode())
    return encrypted.decode()


def decrypt_credential(ciphertext: str) -> Optional[str]:
    """
    Decrypt a stored credential.
    
    Args:
        ciphertext: Base64-encoded encrypted string
        
    Returns:
        Decrypted plaintext, or None if decryption fails
    """
    if not ciphertext:
        return None
    
    try:
        fernet = get_fernet()
        decrypted = fernet.decrypt(ciphertext.encode())
        return decrypted.decode()
    except InvalidToken:
        logger.error("Failed to decrypt credential - invalid token or key mismatch")
        return None
    except Exception as e:
        logger.error(f"Failed to decrypt credential: {e}")
        return None


def is_encrypted(value: str) -> bool:
    """
    Check if a value appears to be encrypted (Fernet format).
    
    Fernet tokens start with 'gAAAAA' when base64 encoded.
    
    Args:
        value: The string to check
        
    Returns:
        True if the value appears to be encrypted
    """
    if not value:
        return False
    return value.startswith('gAAAAA')

