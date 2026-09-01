# Deltabadger

[![Docker Build](https://github.com/deltabadger/deltabadger/actions/workflows/docker-publish.yml/badge.svg)](https://github.com/deltabadger/deltabadger/actions/workflows/docker-publish.yml) [![Docker Image](https://img.shields.io/badge/ghcr.io-deltabadger%2Fdeltabadger-blue?logo=docker)](https://github.com/deltabadger/deltabadger/pkgs/container/deltabadger) [![License](https://img.shields.io/github/license/deltabadger/deltabadger)](LICENSE)

[Deltabadger](https://deltabadger.com) is a beautiful cockpit for long-term investors in stocks and crypto:

* **DCA bots**
* **Portfolio Rebalancing**
* **Direct Indexing**
* **Auto-withdrawals**
* **Portfolio Tracker**
* **Tax Reporting**
* **MCP Server + Rest API**

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

8. [Before you start](manual/08-before-you-start.md)
9. [Create a new bot](manual/09-create-new-bot.md)
10. [Dollar-cost averaging](manual/10-dollar-cost-averaging.md)
11. [Advanced bot settings](manual/11-advanced-bot-settings.md)
12. [Portfolio rebalancing](manual/12-portfolio-rebalancing.md)
13. [Direct indexing](manual/13-direct-indexing.md)
14. [Charts and statistics](manual/14-charts-and-statistics.md)
15. [Managing bots](manual/15-managing-bots.md)

### Automatic withdrawals

16. [Withdrawal rules](manual/16-withdrawal-rules.md)

### Portfolio Tracker

17. [Portfolio Tracker](manual/17-portfolio-tracker.md)
18. [Import and export](manual/18-import-and-export.md)

### Tax reports

19. [Crypto tax report](manual/19-crypto-tax-report.md)
20. [Broker tax report](manual/20-broker-tax-report.md)

### Exchanges and brokers

21. [Supported exchanges](manual/21-supported-exchanges.md)

### API access

22. [MCP server](manual/22-mcp-server.md)
23. [REST API](manual/23-rest-api.md)

### Settings

24. [Account settings](manual/24-account-settings.md)
25. [Two-factor authentication](manual/25-two-factor-authentication.md)
26. [Market data](manual/26-market-data.md)
27. [Multiple users](manual/27-multiple-users.md)
28. [Email notifications](manual/28-email-notifications.md)

### Operations

29. [Configuration](manual/29-configuration.md)
30. [Data and backups](manual/30-data-and-backups.md)
31. [Secrets and encryption keys](manual/31-secrets-and-encryption-keys.md)
32. [Troubleshooting](manual/32-troubleshooting.md)
33. [Development](manual/33-development.md)

## Community

Are you a developer? Jump on the [Telegram channel](https://t.me/deltabadgerchat) and help build the best DCA bot out there.

## License

[AGPL-3.0](LICENSE)
