# Quick start

Install [Docker Desktop](https://www.docker.com/products/docker-desktop/) for your operating system, and make sure it's running, then run Deltabadger with a single command:

```bash
docker run -d --name deltabadger -p 3737:3000 -v deltabadger_data:/app/storage ghcr.io/deltabadger/deltabadger:latest standalone
```

That's it! Access the app at `http://localhost:3737`.

## Self-Hosted plan

If you have a Deltabadger.com Self-Hosted subscription, copy the one-time connect code from your dashboard and pass it when you first start the container:

```bash
docker run -d --name deltabadger -p 3737:3000 -v deltabadger_data:/app/storage -e CLAIM_TOKEN=dbc_your_connect_code ghcr.io/deltabadger/deltabadger:latest standalone
```

Claiming connects subscription market data and exchange proxies, and prefills the editable admin name and email on the setup page. You can also paste the code during setup or connect later under Settings → Connect. A subscription is optional: without a claim, Deltabadger continues through the independent setup path and can use your own CoinGecko key.
