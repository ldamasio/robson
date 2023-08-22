from pathlib import Path
from decouple import config
from binance.client import Client

BINANCE_API_KEY_TEST=config("RBS_BINANCE_API_KEY_TEST")
BINANCE_SECRET_KEY_TEST=config("RBS_BINANCE_SECRET_KEY_TEST")
client=Client(BINANCE_API_KEY_TEST, BINANCE_SECRET_KEY_TEST)

BASE_DIR = Path(__file__).resolve().parent.parent
SECRET_KEY = config("RBS_SECRET_KEY")
DEBUG = True
ALLOWED_HOSTS = ['*']
AUTH_USER_MODEL = 'clients.CustomUser'

INSTALLED_APPS = [
    'django.contrib.admin',
    'django.contrib.auth',
    'django.contrib.contenttypes',
    'django.contrib.sessions',
    'django.contrib.messages',
    'django.contrib.staticfiles',

    'clients.apps.ClientsConfig',
    'api.apps.ApiConfig',

    'corsheaders',
    'rest_framework',
]

REST_FRAMEWORK = {
    'DEFAULT_PERMISSION_CLASSES': [
        'rest_framework.permissions.DjangoModelPermissionsOrAnonReadOnly'
    ]
}

MIDDLEWARE = [
    'django.middleware.security.SecurityMiddleware',
    'django.contrib.sessions.middleware.SessionMiddleware',
    'django.middleware.common.CommonMiddleware',
    'django.middleware.csrf.CsrfViewMiddleware',
    'django.contrib.auth.middleware.AuthenticationMiddleware',
    'django.contrib.messages.middleware.MessageMiddleware',
    'django.middleware.clickjacking.XFrameOptionsMiddleware',

    'corsheaders.middleware.CorsMiddleware',
    'api.middleware.MultitenantMiddleware',
    # 'django_multitenant.middleware.MultitenantMiddleware',
]

ROOT_URLCONF = 'backend.urls'

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

DATABASES = {
    'default': {
        'ENGINE': config('RBS_DATABASES_ENGINE'),
        'NAME': config("RBS_PG_DATABASE"),
        'USER': config("RBS_PG_USER"),
        'PASSWORD': config("RBS_PG_PASSWORD"),
        'HOST': config("RBS_PG_HOST"),
        'PORT': config("RBS_PG_PORT"),
    }
}

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

LANGUAGE_CODE = 'en-us'
TIME_ZONE = 'UTC'
USE_I18N = True
USE_TZ = True

STATIC_URL = 'static/'
DEFAULT_AUTO_FIELD = 'django.db.models.BigAutoField'
CORS_ORIGIN_ALLOW_ALL = True
