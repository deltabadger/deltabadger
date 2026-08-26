# Deltabadger

[![Docker Build](https://github.com/deltabadger/deltabadger/actions/workflows/docker-publish.yml/badge.svg)](https://github.com/deltabadger/deltabadger/actions/workflows/docker-publish.yml) [![Docker Image](https://img.shields.io/badge/ghcr.io-deltabadger%2Fdeltabadger-blue?logo=docker)](https://github.com/deltabadger/deltabadger/pkgs/container/deltabadger) [![License](https://img.shields.io/github/license/deltabadger/deltabadger)](LICENSE)

[Deltabadger](https://deltabadger.com) is a one-stop-shop for investors in crypto and stocks:

No other tool offers this unique combination:

* **DCA bots** for crypto and stocks
* **Auto-withdrawals** to keep your assets safe
* **MCP Server** to connect your exchange accounts to Claude or Claw
* **Crypto Tax Reporting** tool with no transaction limits

For tax-reporting, and some more advanced features you'll need a free CoinGecko account for market data.

## Quick start

Install [Docker Desktop](https://www.docker.com/products/docker-desktop/) for your operating system, make sure it's running, then:

```bash
docker run -d --name deltabadger -p 3737:3000 -v deltabadger_data:/app/storage ghcr.io/deltabadger/deltabadger:latest standalone
```

Open `http://localhost:3737`. See the [manual](#manual) for Docker Compose, the desktop app, Umbrel, and everything else.

## Manual

### Getting started

1. [Welcome to Deltabadger](manual/01-welcome.md)
2. [Quick start](manual/02-quick-start.md)
3. [Docker Compose](manual/03-docker-compose.md)
4. [Desktop app](manual/04-desktop-app.md)
5. [Umbrel](manual/05-umbrel.md)
6. [First run](manual/06-first-run.md)
7. [Updating](manual/07-updating.md)

### Bots

#### DCA

8. [Creating your first bot](manual/08-creating-your-first-bot.md)
9. [Amount and interval](manual/09-amount-and-interval.md)
10. [Order options](manual/10-order-options.md)
11. [Triggers](manual/11-triggers.md)
12. [Selling](manual/12-selling.md)
13. [Managing bots](manual/13-managing-bots.md)
14. [Charts, statistics and orders](manual/14-charts-statistics-and-orders.md)

#### Portfolio Rebalancing

15. [Portfolio bot](manual/15-portfolio-bot.md)
16. [Rebalancing](manual/16-rebalancing.md)

#### Direct Indexing

17. [Index bot](manual/17-index-bot.md)
18. [Index changes](manual/18-index-changes.md)

### Rules

#### Automatic Withdrawals

19. [Withdrawal rules](manual/19-withdrawal-rules.md)
20. [Withdrawal keys and addresses](manual/20-withdrawal-keys-and-addresses.md)

### Tracker

#### Portfolio Tracker

21. [Connecting exchanges](manual/21-connecting-exchanges.md)
22. [Portfolio overview](manual/22-portfolio-overview.md)
23. [Transactions and positions](manual/23-transactions-and-positions.md)
24. [Import and export](manual/24-import-and-export.md)

#### Tax Reports

25. [Crypto tax report](manual/25-crypto-tax-report.md)
26. [Broker tax report](manual/26-broker-tax-report.md)

### Exchanges and brokers

27. [Supported exchanges](manual/27-supported-exchanges.md)
28. [API keys](manual/28-api-keys.md)
29. [Stocks and ETFs](manual/29-stocks-and-etfs.md)

### MCP server

30. [Connecting Claude](manual/30-connecting-claude.md)
31. [Tools and permissions](manual/31-tools-and-permissions.md)

### REST API

32. [REST API](manual/32-rest-api.md)

### Settings

33. [Account settings](manual/33-account-settings.md)
34. [Two-factor authentication](manual/34-two-factor-authentication.md)
35. [Market data](manual/35-market-data.md)
36. [Multiple users](manual/36-multiple-users.md)
37. [Email notifications](manual/37-email-notifications.md)

### Operations

38. [Configuration](manual/38-configuration.md)
39. [Data and backups](manual/39-data-and-backups.md)
40. [Secrets and encryption keys](manual/40-secrets-and-encryption-keys.md)
41. [Troubleshooting](manual/41-troubleshooting.md)
42. [Development](manual/42-development.md)

## Community

Are you a developer? Jump on the [Telegram channel](https://t.me/deltabadgerchat) and help build the best DCA bot out there.

## License

[AGPL-3.0](LICENSE)
