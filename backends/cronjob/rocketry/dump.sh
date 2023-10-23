filepath=/app/$(date +%F_%H-%M-%S).sql
pg_dump -Fc --host=$POSTGRES_HOST --port=$POSTGRES_PORT --username=$POSTGRES_USER --password=$POSTGRES_PASSWORD --dbname=$POSTGRES_DBNAME -f filename
cp $filepath /app/pg-dump.sql
