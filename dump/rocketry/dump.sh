filepath=/app/$(date +%F_%H-%M-%S).sql
pg_dump -Fc --host=$PG_HOST --port=$PG_PORT --username=$PG_USER --password=$PG_PASSWORD --dbname=$PG_DBNAME -f filename
cp $filepath /app/pg-dump.sql
