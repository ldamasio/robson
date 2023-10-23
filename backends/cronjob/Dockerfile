FROM ubuntu:latest
RUN apt-get update; apt-get install -y postgresql-client python3 python3-pip
WORKDIR /app
ENV TZ=America/Sao_Paulo
RUN ln -snf /usr/share/zoneinfo/$TZ /etc/localtime && echo $TZ > /etc/timezone
RUN pip3 install rocketry
COPY ./rocketry/main.py /app/rocketry/main.py
COPY ./rocketry/dump.sh /app/rocketry/dump.sh
COPY ./rocketry/backup.sh /app/rocketry/backup.sh
CMD ["python3", "/app/rocketry/main.py"]
