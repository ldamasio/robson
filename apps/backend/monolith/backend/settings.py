# backend/settings.py - CLEAN AND TESTED VERSION
from pathlib import Path
from decouple import AutoConfig
from datetime import timedelta
import os
import sys

BASE_DIR = Path(__file__).resolve().parent.parent
config = AutoConfig(search_path=BASE_DIR)

# Ensure repository root is in sys.path so `apps.*` packages are importable
REPO_ROOT = BASE_DIR.parent.parent.parent  # <repo>/
if str(REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(REPO_ROOT))

# ==========================================
# BASIC CONFIGURATION
# ==========================================
SECRET_KEY = config("RBS_SECRET_KEY")
DEBUG = config('DEBUG', default=False, cast=bool)
ALLOWED_HOSTS = ['*']
AUTH_USER_MODEL = 'clients.CustomUser'

# ==========================================
# TRADING/BINANCE CONFIGURATION
# ==========================================
# Testnet credentials (for development and testing)
BINANCE_API_KEY_TEST = config("RBS_BINANCE_API_KEY_TEST", default="")
BINANCE_SECRET_KEY_TEST = config("RBS_BINANCE_SECRET_KEY_TEST", default="")

# Production credentials (for real trading)
# Note: K8s uses RBS_BINANCE_API_KEY_PROD, we support both naming conventions
BINANCE_API_KEY = config("RBS_BINANCE_API_KEY_PROD", default=config("RBS_BINANCE_API_KEY", default=""))
BINANCE_SECRET_KEY = config("RBS_BINANCE_SECRET_KEY_PROD", default=config("RBS_BINANCE_SECRET_KEY", default=""))

# Trading flags
BINANCE_USE_TESTNET = config('BINANCE_USE_TESTNET', default=True, cast=bool)
TRADING_ENABLED = config('TRADING_ENABLED', default=False, cast=bool)

# Encryption key for client credentials stored in database
# Uses Django's SECRET_KEY if not explicitly provided
CREDENTIAL_ENCRYPTION_KEY = config("RBS_CREDENTIAL_ENCRYPTION_KEY", default="")

# ==========================================
# DJANGO APPS
# ==========================================
DJANGO_APPS = [
    'django.contrib.admin',
    'django.contrib.auth',
    'django.contrib.contenttypes',
    'django.contrib.sessions',
    'django.contrib.messages',
    'django.contrib.staticfiles',
]

THIRD_PARTY_APPS = [
    'corsheaders',
    'rest_framework',
    'rest_framework_simplejwt.token_blacklist',
    'django_extensions',  # ACTIVE for runserver_plus
]

LOCAL_APPS = [
    'clients.apps.ClientsConfig',
    'api.apps.ApiConfig',
]

INSTALLED_APPS = DJANGO_APPS + THIRD_PARTY_APPS + LOCAL_APPS

# ==========================================
# REST FRAMEWORK
# ==========================================
REST_FRAMEWORK = {
    'DEFAULT_AUTHENTICATION_CLASSES': (
        'rest_framework_simplejwt.authentication.JWTAuthentication',
    ),
    'DEFAULT_PERMISSION_CLASSES': [
        'rest_framework.permissions.IsAuthenticated',
    ],
    'DEFAULT_PAGINATION_CLASS': 'rest_framework.pagination.PageNumberPagination',
    'PAGE_SIZE': 20,
    'DEFAULT_THROTTLE_CLASSES': [
        'rest_framework.throttling.AnonRateThrottle',
        'rest_framework.throttling.UserRateThrottle'
    ],
    'DEFAULT_THROTTLE_RATES': {
        'anon': '100/hour',
        'user': '1000/hour',
        'login': '5/min',
    }
}

# ==========================================
# JWT SETTINGS
# ==========================================
SIMPLE_JWT = {
    "ACCESS_TOKEN_LIFETIME": timedelta(hours=1),
    "REFRESH_TOKEN_LIFETIME": timedelta(days=30),
    "ROTATE_REFRESH_TOKENS": True,
    "BLACKLIST_AFTER_ROTATION": True,
    "UPDATE_LAST_LOGIN": True,
    "ALGORITHM": "HS256",
    "AUTH_HEADER_TYPES": ("Bearer",),
    "AUTH_HEADER_NAME": "HTTP_AUTHORIZATION",
    "USER_ID_FIELD": "id",
    "USER_ID_CLAIM": "user_id",
    "AUTH_TOKEN_CLASSES": ("rest_framework_simplejwt.tokens.AccessToken",),
    "TOKEN_TYPE_CLAIM": "token_type",
    "JTI_CLAIM": "jti",
}

# ==========================================
# MIDDLEWARE
# ==========================================
MIDDLEWARE = [
    # Prometheus metrics - must be first
    'django_prometheus.middleware.PrometheusBeforeMiddleware',

    # Correlation ID - early for request tracking
    'api.middleware.correlation_id.CorrelationIDMiddleware',

    # Kubernetes probe endpoints - bypass SSL redirect BEFORE SecurityMiddleware
    'api.middleware.probe_no_redirect.ProbeNoRedirectMiddleware',

    'corsheaders.middleware.CorsMiddleware',
    'django.middleware.security.SecurityMiddleware',
    'django.contrib.sessions.middleware.SessionMiddleware',
    'django.middleware.common.CommonMiddleware',
    'django.middleware.csrf.CsrfViewMiddleware',
    'django.contrib.auth.middleware.AuthenticationMiddleware',
    'django.contrib.messages.middleware.MessageMiddleware',
    'django.middleware.clickjacking.XFrameOptionsMiddleware',

    # Prometheus metrics - must be last
    'django_prometheus.middleware.PrometheusAfterMiddleware',
]

ROOT_URLCONF = 'backend.urls'

# ==========================================
# TEMPLATES
# ==========================================
TEMPLATES = [
    {
        'BACKEND': 'django.template.backends.django.DjangoTemplates',
        'DIRS': [],
        'APP_DIRS': True,
        'OPTIONS': {
            'context_processors': [
                'django.template.context_processors.debug',
                'django.template.context_processors.request',
                'django.contrib.auth.context_processors.auth',
                'django.contrib.messages.context_processors.messages',
            ],
        },
    },
]

WSGI_APPLICATION = 'backend.wsgi.application'

# ==========================================
# DATABASES
# ==========================================
DATABASES = {
    'default': {
        'ENGINE': 'django.db.backends.postgresql_psycopg2',
        'NAME': config('RBS_PG_DATABASE'),
        'USER': config('RBS_PG_USER'),
        'PASSWORD': config('RBS_PG_PASSWORD'),
        'HOST': config('RBS_PG_HOST'),
        'PORT': config('RBS_PG_PORT'),
    }
}

# ==========================================
# CACHE
# ==========================================
REDIS_URL = config("REDIS_URL", default="")

if REDIS_URL:
    CACHES = {
        'default': {
            'BACKEND': 'django_redis.cache.RedisCache',
            'LOCATION': REDIS_URL,
            'OPTIONS': {
                'CLIENT_CLASS': 'django_redis.client.DefaultClient',
            },
            'TIMEOUT': 1,
        }
    }
else:
    CACHES = {
        'default': {
            'BACKEND': 'django.core.cache.backends.locmem.LocMemCache',
            'LOCATION': 'robson-cache',
            'TIMEOUT': 1,
        }
    }

# ==========================================
# LOGGING
# ==========================================
LOGGING = {
    'version': 1,
    'disable_existing_loggers': False,
    'formatters': {
        'verbose': {
            'format': '{levelname} {asctime} {module} {process:d} {thread:d} {message}',
            'style': '{',
        },
        'simple': {
            'format': '{levelname} {message}',
            'style': '{',
        },
        'json': {
            '()': 'pythonjsonlogger.jsonlogger.JsonFormatter',
            'format': '%(asctime)s %(name)s %(levelname)s %(message)s %(pathname)s %(lineno)d %(funcName)s %(correlation_id)s',
        },
    },
    'filters': {
        'correlation_id': {
            '()': 'api.middleware.logging_filter.CorrelationIDFilter',
        },
    },
    'handlers': {
        'file': {
            'level': 'INFO',
            'class': 'logging.FileHandler',
            'filename': BASE_DIR / 'logs' / 'robson.log',
            'formatter': 'verbose',
            'filters': ['correlation_id'],
        },
        'console': {
            'level': 'DEBUG' if DEBUG else 'INFO',
            'class': 'logging.StreamHandler',
            'formatter': 'simple' if DEBUG else 'json',  # JSON in production for k8s
            'filters': ['correlation_id'],
        },
    },
    'loggers': {
        'api': {
            'handlers': ['file', 'console'],
            'level': 'INFO',
            'propagate': True,
        },
        'api.views.auth': {
            'handlers': ['file', 'console'],
            'level': 'INFO',
            'propagate': True,
        },
        'binance': {
            'handlers': ['file', 'console'],
            'level': 'WARNING',
            'propagate': True,
        },
        'trading': {
            'handlers': ['file', 'console'],
            'level': 'INFO',
            'propagate': True,
        },
        'rest_framework_simplejwt': {
            'handlers': ['file', 'console'],
            'level': 'WARNING',
            'propagate': True,
        },
    },
}

# Create logs directory if it doesn't exist
os.makedirs(BASE_DIR / 'logs', exist_ok=True)

# ==========================================
# PASSWORD CONFIGURATION
# ==========================================
AUTH_PASSWORD_VALIDATORS = [
    {
        'NAME': 'django.contrib.auth.password_validation.UserAttributeSimilarityValidator',
    },
    {
        'NAME': 'django.contrib.auth.password_validation.MinimumLengthValidator',
    },
    {
        'NAME': 'django.contrib.auth.password_validation.CommonPasswordValidator',
    },
    {
        'NAME': 'django.contrib.auth.password_validation.NumericPasswordValidator',
    },
]

# ==========================================
# INTERNATIONALIZATION
# ==========================================
LANGUAGE_CODE = 'en-us'
TIME_ZONE = 'UTC'
USE_I18N = True
USE_TZ = True

# ==========================================
# STATIC FILES
# ==========================================
STATIC_ROOT = 'staticfiles'
STATIC_URL = 'static/'
DEFAULT_AUTO_FIELD = 'django.db.models.BigAutoField'

# ==========================================
# CORS CONFIGURATION
# ==========================================
if DEBUG:
    CORS_ALLOW_ALL_ORIGINS = True
    CORS_ALLOW_CREDENTIALS = True
else:
    CORS_ALLOWED_ORIGINS = [
        # Production domains (rbx.ia.br)
        "https://api.robson.rbx.ia.br",
        "https://app.robson.rbx.ia.br",
        "https://robson.rbx.ia.br",
        # Legacy domains (robsonbot.com) - keep for now
        "https://backend.robsonbot.com",
        "https://app.robsonbot.com",
        "https://www.robsonbot.com",
    ]
    CORS_ALLOW_CREDENTIALS = True

CORS_ALLOW_HEADERS = [
    'accept',
    'accept-encoding',
    'authorization',
    'content-type',
    'dnt',
    'origin',
    'user-agent',
    'x-csrftoken',
    'x-requested-with',
    'x-client-id',
]

CORS_ALLOW_METHODS = [
    'DELETE',
    'GET',
    'OPTIONS',
    'PATCH',
    'POST',
    'PUT',
]

CORS_EXPOSE_HEADERS = [
    'content-type',
    'x-csrf-token',
]

# ==========================================
# CSRF CONFIGURATION
# ==========================================
CSRF_TRUSTED_ORIGINS = [
    # Production domains
    "https://api.robson.rbx.ia.br",
    "https://app.robson.rbx.ia.br",
    "https://robson.rbx.ia.br",
    # Legacy domains
    "https://backend.robsonbot.com",
    # Development
    "http://localhost:3000",
    "http://127.0.0.1:3000",
    "https://localhost:3000",
    "https://127.0.0.1:3000",
    "http://localhost:5173",
    "http://127.0.0.1:5173",
    "https://localhost:5173",
    "https://127.0.0.1:5173",
    "https://localhost:8000",
    "https://127.0.0.1:8000",
]

# ==========================================
# SECURITY SETTINGS
# ==========================================
if DEBUG:
    CSRF_COOKIE_SECURE = False
    CSRF_COOKIE_HTTPONLY = True
    CSRF_COOKIE_SAMESITE = 'Lax'
    SESSION_COOKIE_SECURE = False
    SESSION_COOKIE_HTTPONLY = True
    SESSION_COOKIE_SAMESITE = 'Lax'
    SECURE_SSL_REDIRECT = False
else:
    CSRF_COOKIE_SECURE = True
    CSRF_COOKIE_HTTPONLY = True
    CSRF_COOKIE_SAMESITE = 'Lax'
    SESSION_COOKIE_SECURE = True
    SESSION_COOKIE_HTTPONLY = True
    SESSION_COOKIE_SAMESITE = 'Lax'
    SECURE_SSL_REDIRECT = True
    SECURE_HSTS_SECONDS = 31536000
    SECURE_HSTS_INCLUDE_SUBDOMAINS = True
    SECURE_HSTS_PRELOAD = True
    SECURE_BROWSER_XSS_FILTER = True
    SECURE_CONTENT_TYPE_NOSNIFF = True

# Trusted proxy header for HTTPS detection
SECURE_PROXY_SSL_HEADER = ('HTTP_X_FORWARDED_PROTO', 'https')

# ==========================================
# DJANGO EXTENSIONS CONFIGURATION
# ==========================================
if DEBUG:
    SHELL_PLUS_PRINT_SQL = True
    SHELL_PLUS_IMPORTS = [
        'from clients.models import *',
        'from api.models import *',
        'from decimal import Decimal',
        'from datetime import datetime, timedelta',
    ]

# ==========================================
# ROBSON BOT CONFIGURATION
# ==========================================
ROBSON_BOT = {
    'MARKET_DATA_CACHE_TIMEOUT': 300,
    'PRICE_UPDATE_INTERVAL': 1,
    'MAX_ORDERS_PER_MINUTE': 10,
    'DEFAULT_STOP_LOSS_PERCENT': 2.0,
    'DEFAULT_TAKE_PROFIT_PERCENT': 4.0,
    'MAX_POSITION_SIZE_PERCENT': 5.0,
    'MAX_DAILY_LOSS_PERCENT': 10.0,
    'HEALTH_CHECK_INTERVAL': 30,
    'ALERT_WEBHOOKS': [],
}

# ==========================================
# FRONTEND CONFIGURATION
# ==========================================
FRONTEND_CONFIG = {
    'API_BASE_URL': '/api/',
    'AUTH_ENDPOINTS': {
        'LOGIN': '/api/auth/token/',
        'REFRESH': '/api/auth/token/refresh/',
        'VERIFY': '/api/auth/token/verify/',
        'LOGOUT': '/api/auth/token/blacklist/',
        'USER_PROFILE': '/api/user/',
    },
    'TRADING_ENDPOINTS': {
        'STRATEGIES': '/api/strategies/',
        'ORDERS': '/api/orders/',
        'PLACE_ORDER': '/api/orders/place/',
        'BALANCE': '/api/account/balance/',
        'PATRIMONY': '/api/portfolio/patrimony/',
    },
    'WEBSOCKET_URL': 'ws://localhost:8000/ws/' if DEBUG else 'wss://backend.robsonbot.com/ws/',
}

# ==========================================
# DEVELOPMENT SETTINGS
# ==========================================
if DEBUG:
    # Add database logging in development
    LOGGING['loggers']['django.db.backends'] = {
        'handlers': ['console'],
        'level': 'DEBUG',
        'propagate': False,
    }
    
    # Development info
    print("🚀 Robson Bot - Development Mode")
    print(f"📊 Django Extensions: {'✅ Active' if 'django_extensions' in INSTALLED_APPS else '❌ Inactive'}")
    print(f"🔒 HTTPS Support: {'✅ Available' if 'django_extensions' in INSTALLED_APPS else '❌ Not available'}")
    print(f"🌐 CORS: {'✅ All origins allowed' if CORS_ALLOW_ALL_ORIGINS else '❌ Restricted'}")
    print(f"🔑 JWT Lifetime: {SIMPLE_JWT['ACCESS_TOKEN_LIFETIME']}")
    print("-" * 50)
