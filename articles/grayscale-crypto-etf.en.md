---
title: "Decoding Grayscale's <span class=nobreak>Multi-Crypto ETF</span>"
subtitle: "Index Universe, CD20, CD5—CoinDesk Indices Stealthily Shape the Market"
author_id: 1
thumbnail: grayscale-etf.avif
excerpt: "Why does Grayscale's new ETF allocate 80% to Bitcoin when pure market cap would suggest 73%? The answer lies in CoinDesk's sophisticated four-layer indexing methodology—and what it means for crypto investing."
x_url: https://x.com/deltabadgerapp/status/1939622461768892684
telegram_url: https://t.me/deltabadger/91
published: true
---

## The Era of Crypto Indices Begins

On July 1, 2025, the SEC [opened a new chapter for crypto investing](https://cointelegraph.com/news/sec-conversion-grayscale-large-cap-crypto-fund-etf) by approving the transformation of the Grayscale Digital Large Cap Fund (GDLC) into a spot Exchange-Traded Fund (ETF), which will be trading on NYSE Arca with $775 million in assets under management.

For Grayscale, this marks a significant shift from the closed-end fund structure that previously limited investor access. Now, both institutional and retail investors can gain crypto exposure through a familiar, regulated vehicle without the need to manage wallets or private keys.

Unfortunately, as with other American ETFs, investors from the EU will have limited access, typically requiring the status of a professional investor or the use of a broker outside the EU zone.

What captures attention isn't just the size, but the composition:

## From Index Universe to CoinDesk 5

The ETF offers diversified exposure across major cryptocurrencies, with **Bitcoin dominating at ~80%** of the portfolio, complemented by **Ethereum, XRP, Solana, and Cardano**.

While most assume the ETF simply tracks the top 5 cryptocurrencies by market cap, it actually follows the **CoinDesk 5 Index (CD5)**—a complex methodology designed for institutional investors, providing subtle hints about where institutional money might flow next, making it worth a closer look.

CD5 sits at the top of a **four-tier methodology** developed by [CoinDesk Indices](https://indices.coindesk.com/). Unlike popular market cap rankings on sites like CoinGecko or CoinPaprika, CD5 was built specifically for institutional investors. This means its complex methodology may surprise crypto enthusiasts—it's far more selective than simply picking the largest coins by market cap.

{::nomarkdown}
<figure class="article__figure">
<img src="/assets/articles/coindesk-methodology.svg" alt="The four layers of CoinDesk Methodology">
<figcaption class="article__figure__caption">The four tiers of CoinDesk Methodology</figcaption>
</figure>
{:/nomarkdown}

### The Index Universe

Everything begins with the **Index Universe**—CoinDesk's foundational methodology for determining which cryptocurrencies are even worth considering for institutional investment.

The Index Universe starts with the **top 250 cryptocurrencies by market capitalization**, then applies a set of strict quality filters:

<!-- PAYWALL -->

**What gets eliminated immediately:**

❌ **Stablecoins** (USDT, USDC) - they're meant to stay flat, not grow  
❌ **Memecoins** (yes, even if they're worth billions) - too volatile and speculative  
❌ **Wrapped or staked tokens** - these are derivatives, not the underlying assets  
❌ **Privacy coins** - regulatory concerns make them institutional no-gos  
❌ **Securities** - anything that might be classified as a security under US law  

**What survives must prove liquidity:**

✅ Listed on at least 3 major exchanges with USD/USDC pairs  
✅ At least one listing must be 90+ days old (no brand-new tokens)  
✅ Active trading in the past 30 days across multiple exchanges  
✅ Available to US customers on at least one exchange  
✅ Sufficient median daily trading volume (the midpoint of daily trading activity over 90 days)  

This filtering process creates the Index Universe—a curated list of cryptocurrencies that meet institutional investment standards. While CoinDesk doesn't publicly share the exact number of cryptocurrencies that make it through these filters, it's likely around 50 coins—we only know for sure it's more than 20 but less than 250.

This lack of transparency around the actual Index Universe size is one of the current limitations, along with CD5's still-developing documentation. Greater clarity on these numbers would help investors better understand the nature of the index.

### CoinDesk 20

The [CoinDesk 20 Index (CD20)](https://indices.coindesk.com/coindesk20) takes this vetted universe and adds a further portfolio management layer. It's also the best documented online part of the offering.

**Market cap weighting with guardrails**: The market cap weighting of the index, which Deltabadger users know from the [rebalanced DCA bot](https://deltabadger.com/academy/rebalanced-dca/), is adjusted by adding caps (30% max for the largest, 20% for others). It's not really clear why the cap has been added, and the official CoinDesk documentation doesn't explain it.

{::nomarkdown}
<figure class="article__figure" data-controller="pie-chart" data-pie-chart-data-value="#F7931A,BTC,Bitcoin,30.53
#7349A4,ETH,Ethereum,24.83
#2F2C56,XRP,XRP,18.25
#00FFA3,SOL,Solana,11.84
#0045D0,ADA,Cardano,3.05
#8DC351,BCH,Bitcoin Cash,1.50
#4DA6FF,SUI,Sui,1.33
#375BD2,LINK,Chainlink,1.29
#E84142,AVAX,Avalanche,1.10
#7D00FF,XLM,Stellar,1.08
COLUMN_BREAK
#BFBBBB,LTC,Litecoin,0.96
#40826D,HBAR,Hedera,0.93
#FF007A,UNI,Uniswap,0.65
#B6509E,AAVE,Aave,0.61
#000000,APT,Aptos,0.44
#29ABE2,ICP,Internet Computer,0.39
#00C08B,NEAR,Near,0.38
#E6007A,DOT,Polkadot,0.32
#8247E5,MATIC,Polygon,0.24
#0090FF,FIL,Filecoin,0.22">
<div class="pie-chart-wrapper">
<svg data-pie-chart-target="svg" width="300" height="300" class="pie-chart"></svg>
<div data-pie-chart-target="legend" class="pie-legend"></div>
</div>
<figcaption class="article__figure__caption">CoinDesk 20 Index (CD20) Allocation</figcaption>
</figure>
{:/nomarkdown}

**Quarterly rebalancing**: Every three months, the entire portfolio gets recalibrated based on current market conditions, with "buffer rules" to prevent excessive turnover.

The rules are designed to prevent excessive turnover by giving current index constituents a preference to stay in the index even if they've dropped slightly in the rankings.

CD20 uses a **buffer system**:

**Entry Criteria**:
1. An asset must rank in the top **40** by trading volume to be considered  
2. **Top 15 spots**:  
- Go to the largest assets by market cap (regardless of current status)  
3. **Remaining 5 spots**:  
- Current constituents ranked 16-25 by market cap get preference  
- If not enough current constituents qualify, highest-ranked new assets fill remaining spots  

**Exit Criteria**: An existing asset loses its spot if it either:  
1. Falls below rank **50** in trading volume (liquidity failure), OR  
2. Falls below rank **25** in market cap (size failure)  

Without buffer rules, assets hovering around the cutoff line would constantly be added and removed each quarter, creating unnecessary transaction costs and instability.

### CoinDesk 5

However, it's not the CD20 that is being used for the first multi-crypto ETF, but its younger sibling:

**CD5** represents the top 5 assets from CD20, selected by **market capitalization** from the already-vetted CD20 constituents. Since CD20 already applies strict liquidity filters (including the trading volume screening), CD5 inherits these quality standards while focusing on the largest, most liquid assets.

The index is much younger and still lacks equal online coverage, but studying its construction, it's clear it was meant to be used by institutions.

The key difference from CD20 is that **the caps are removed**. While CD20 limits the largest constituent to 30% and others to 20%, CD5 uses pure market capitalization weighting. This results in Bitcoin currently reaching over 80% of the allocation—a reality check that reflects Bitcoin's dominant market position and liquidity profile. This concentration is a feature, not a bug—it ensures the index reflects the true market capitalization hierarchy of the most liquid crypto assets.

Like CD20, CD5 follows the same quarterly rebalancing schedule but uses a simpler **4/6 buffer rule**: the top 4 assets by market cap from CD20 are automatically included, while existing constituents ranked 5th or 6th get preference to remain. This streamlined process makes CD5 particularly suitable for ETF implementation, as it reduces complexity while maintaining rigorous institutional standards.

{::nomarkdown}
<figure class="article__figure" data-controller="pie-chart" data-pie-chart-data-value="#F7931A,BTC,Bitcoin,79.35
#7349A4,ETH,Ethereum,10.63
#2F2C56,XRP,Ripple,5.78
#00FFA3,SOL,Solana,3.09
#0045D0,ADA,Cardano,1.14">
<div class="pie-chart-wrapper">
<svg data-pie-chart-target="svg" width="300" height="300" class="pie-chart"></svg>
<div data-pie-chart-target="legend" class="pie-legend"></div>
</div>
<figcaption class="article__figure__caption">CoinDesk 5 Index (CD5) Allocation</figcaption>
</figure>
{:/nomarkdown}

### Summary of the Index Structure

To summarize the methodology:

**Index Universe** - establishes baseline eligibility criteria, filters around 50 assets (?).  
**CD20** - applies portfolio management principles including market cap weighting with caps, trading volume screening, and quarterly rebalancing with buffer rules.  
**CD5** - selects the top 5 assets from CD20 by market cap, removes caps, and uses simplified buffer rules optimized for institutional investment.

## The Self-Fulfilling Prophecy

While it's clear that institutional investors are interested in the broader crypto market, only a few digital assets so far meet the strict liquidity and regulatory criteria required for ETF inclusion.

Looking closer at the indices structure, we're getting hints about where big capital will be flowing in the near future. The four-tier methodology isn't just about risk management—it's a roadmap for institutional capital allocation.

This is important because many criticize indices as becoming self-fulfilling prophecies leading to further concentration of capital. When massive ETF inflows chase the same limited set of assets, prices inflate regardless of fundamentals.

Michael Burry famously argued that passive investing, like S&P 500 ETFs, inflates stock prices through massive capital inflows without fundamental analysis—similar to a Ponzi scheme's reliance on new investors. He warned this could lead to a liquidity crisis when outflows occur, as the market's "exit door" is limited.

On the other hand, since his last short, the index is up. Will it work the same for the cryptocurrency market?

The trade idea here is simple: **invest into what's in the ETF**. If institutional capital flows into CD5 constituents, their prices should benefit from this structural demand—regardless of whether you believe in the underlying technology.

### The Ripple Effect?

ETF Store President [Nate Geraci sees broader implications](https://twitter.com/NateGeraci/status/1939454629915619403), suggesting this approval could pave the way for individual spot ETFs for assets like XRP, Solana, and Litecoin. This would allow investors to gain targeted exposure to specific cryptocurrencies through traditional investment accounts.

<blockquote class="twitter-tweet">
<p lang="en" dir="ltr">Final SEC deadline this week on Grayscale Digital Large Cap ETF (GDLC)…<br><br>Holds btc, eth, xrp, sol, &amp; ada.<br><br>Think *high likelihood* this is approved.<br><br>Would then be followed later by approval for individual spot ETFs on xrp, sol, ada, etc.</p>&mdash; Nate Geraci (@NateGeraci) <a href="https://twitter.com/NateGeraci/status/1939454629915619403?ref_src=twsrc%5Etfw">June 29, 2025</a>
</blockquote>

## Looking Ahead

Each regulatory milestone signals the crypto market's continued maturation. The SEC's approval of diversified crypto ETFs opens the era of crypto indices.

As we know from traditional markets, indices in the form of ETFs and mutual funds are the main way people invest in the market. So far, the cryptocurrency market was lacking this most obvious approach.

However, the launch of the GDLC ETF creates an opportunity gap—while institutional investors gain access to professional crypto indexing through regulated ETFs, individual investors face barriers requiring professional investor status and other extra steps. For most readers, GDLC will likely remain out of reach.

### Our Mission

This accessibility gap is where Deltabadger comes in. Our mission is to make indexing strategies accessible to everyone, with key advantages:

First, individuals are not constrained by liquidity requirements like ETFs are. This gives us more flexibility in index construction and implementation. Index investing in emerging trends like memecoins while maintaining a professional, passive approach is within reach.

Second, we're building a powerful indexing engine (launching later this year) that will give users unprecedented control. With access to over 500 indices from CoinGecko and customizable weightings, you'll be able to build portfolios that precisely match your strategy.

This automated approach eliminates the need for manual management and arbitrary decisions. Just as index investing through ETFs and mutual funds has become the standard in traditional markets, we believe automated indexing will become the default for long-term crypto investing.

Indices' biggest advantage is that they provide natural risk management—failed projects drop out automatically, similar to how bankrupt companies exit the S&P 500. And unlike CoinDesk's quarterly rebalancing, custom indices can adapt more quickly as market conditions change.

-

*What's your take on these developments? Would you invest in CD5, CD20, or just stick to Bitcoin? ETF or self-custodied custom index portfolio?*