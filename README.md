
Robson Bot

Just another crypto robot

ROBSON BOT is an open source algo trade project. It is a robot specialized in cryptocurrency trading (automatic buying and selling of digital assets), programmed, with backend and data modeling in Python, to monitor the market in real time, using asynchronous communication between the exchange and the application, that is, your dashboard and your “brain”. With this, Robson Bot is capable of making intelligent decisions based on a set of strategies guided by probabilistic analysis and technical analysis. The open source project includes a risk management system, tools for disseminating trade signals and functions as a platform, enabling multiple users with security and data isolation (multi-tenant).

The Robson Bot is a tool for researchers, traders that monitors stocks to trigger signals or automate order flows for the binance crypto stock market.

## Research, communication and trade functions.

Designed as a cryptocurrency robot, it also has the ability to communicate and interact via Metaverse, providing services and remuneration to its users, with instructions for risk management.

## Command interface

The command interface makes it possible to activate a Dashboard with its main indicators or special features for you to carry out day-to-day activities.

## The Dashboard offers special string conversion calculators. 

For example, if you need to withdraw an amount of BRL, but would like to convert your USDT to ADA before transferring, in addition to needing to anticipate spread values from other financial services.

## INSTALL

Some tips for development environment

### Clone robson repository

git clone https://github.com/ldamasio/robson.git

### Try run docker-compose

docker-compose up -d --build

### Development Backend Environment

Commands that may be util:

```
cd backends/monolith/
cp .env.example .env
python -m venv .venv
python -m pip install -r requirements.txt
export DJANGO_SETTINGS_MODULE=backend.settings
cp -r staticfiles/* docker/static/
```

### Development Frontend Environment

cd frontends/web
nvm use 14
npm i
npm start

