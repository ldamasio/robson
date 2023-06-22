FROM python:3.8
ENV PYTHONDONTWRITEBYTECODE 1
ENV PYTHONUNBUFFERED 1
COPY . /app-backend
WORKDIR /app-backend
RUN pip3 install -r requirements.txt
CMD ["python", "manage.py", "makemigrations"]
CMD ["python", "manage.py", "migrate"]
CMD gunicorn -b 0.0.0.0:8000 --worker-class=gevent --worker-connections=1000 --workers=5 backend.wsgi
