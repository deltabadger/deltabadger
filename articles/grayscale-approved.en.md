---
title: "Decoding Grayscale's <span class=nobreak>Multi-Crypto ETF</span>"
subtitle: "From Index Universe to CD5 — How CoinDesk Indices Stealthily Shape The Market"
author_id: 1
thumbnail: grayscale-etf.avif
excerpt: "Why does Grayscale's new ETF allocate 80% to Bitcoin when pure market cap would suggest 73%? The answer lies in CoinDesk's sophisticated three-tier indexing methodology—and what it means for crypto investing."
x_url: https://x.com/deltabadgerapp/status/1939622461768892684
telegram_url: https://t.me/deltabadger/91
published: true
---

On July 1, 2025, the SEC [opened a new chapter for crypto investing](https://cointelegraph.com/news/sec-conversion-grayscale-large-cap-crypto-fund-etf) by approving Grayscale's spot ETF—a move that could reshape the digital assets landscape forever.

The approval transformed Grayscale's Digital Large Cap Fund into a spot Exchange-Traded Fund, now trading on NYSE Arca with $775 million in assets under management. What captures attention isn't just the size, but the composition: this isn't another Bitcoin-only fund.

## A True Multi-Crypto Portfolio

The ETF offers diversified exposure across major cryptocurrencies, with **Bitcoin dominating at ~80%** of the portfolio, complemented by **Ethereum, XRP, Solana, and Cardano**.

For Grayscale, this marks a significant shift from the closed-end fund structure that previously limited investor access. Now, both institutional and retail investors can gain crypto exposure through a familiar, regulated vehicle without managing wallets or private keys.

The [official SEC order](https://www.sec.gov/files/rules/sro/nysearca/2025/34-103364.pdf) and Grayscale's [amended S-3 filing](https://www.sec.gov/Archives/edgar/data/1729997/000095017025090512/gdlc_s-3_amendment_3.htm) provide the regulatory framework that makes this possible.

### The Elephant in The Room

While most assume the ETF simply tracks the top 5 cryptocurrencies by market cap, it actually follows the **CoinDesk 5 Index (CD5)**—a complex methodology that's far from neutral. As these indexes gain adoption, they provide subtle hints about where institutional money might flow next, making them worth studying closely.

## From Index Universe to CoinDesk 5

CD5 sits at the top of a three-tier methodology developed by [CoinDesk Indices](https://indices.coindesk.com/). Unlike popular market cap rankings on sites like CoinGecko or CoinPaprika, CD5 was built specifically for institutional investors. This means its sophisticated methodology may surprise crypto enthusiasts—it's far more selective than simply picking the largest coins by market cap.

### The Index Universe: Institutional Filter

Everything begins with the **Index Universe**—CoinDesk's foundational methodology for determining which cryptocurrencies are even worth considering for institutional investment.

The Index Universe starts with the **top 250 cryptocurrencies by market capitalization**, then applies a set of strict quality filters:

<!-- PAYWALL -->

**What gets eliminated immediately:**
- **Stablecoins** (USDT, USDC) - they're meant to stay flat, not grow
- **Memecoins** (yes, even if they're worth billions) - too volatile and speculative  
- **Wrapped or staked tokens** - these are derivatives, not the underlying assets
- **Privacy coins** - regulatory concerns make them institutional no-gos
- **Securities** - anything that might be classified as a security under US law

**What survives must prove liquidity:**
- Listed on at least 3 major exchanges with USD/USDC pairs
- At least one listing must be 90+ days old (no brand-new tokens)
- Active trading in the past 30 days across multiple exchanges
- Available to US customers on at least one exchange

This filtering process creates the Index Universe—a curated list of cryptocurrencies that meet institutional investment standards. While CoinDesk doesn't publicly share the exact number of cryptocurrencies that make it through these filters, it's likely around 50 coins—we only know for sure it's more than 20 but less than 250. 

This lack of transparency around the actual Index Universe size is one of the current limitations, along with CD5's still-developing documentation. Greater clarity on these numbers would help investors better understand the nature of the index.

### CoinDesk 20: Management and Stability

The [CoinDesk 20 Index (CD20)](https://indices.coindesk.com/coindesk20) takes this vetted universe and adds the further portfolio management layer. It's also the best documented online part of the offering. It's hard to avoid the conclusion that the initial hope was the first professional instrument would be built based on CD20. We'll come back to it.

**Market cap weighting with guardrails**: The market cap weighting of the index, which Deltabadger users know from the [rebalanced DCA bot](https://deltabadger.com/academy/rebalanced-dca/), is adjusted by adding caps (30% max for the largest, 20% for others). It's not really clear why the cap has been added, and the official CoinDesk documentation doesn't explain it.

**Quarterly rebalancing**: Every three months, the entire portfolio gets recalibrated based on current market conditions, with "buffer rules" to prevent excessive turnover. 

The rules are designed to prevent excessive turnover by giving current index constituents a preference to stay in the index even if they've dropped slightly in the rankings. Without buffer rules, assets hovering around the cutoff line would constantly be added and removed each quarter, creating unnecessary transaction costs and instability.

CD20 uses a 40/50 buffer rule:
- **New assets** (non-constituents): Must rank in the top 40 to be considered for inclusion
- **Existing assets** (current constituents): Can rank as low as 50 and still be considered for inclusion

The selection process prioritizes the top 15 spots for the highest-ranked assets, then gives current constituents ranked 16-25 preference to fill the remaining 5 spots. This creates stability while still allowing natural evolution over time.

### CoinDesk 5: The Great Liquidity Filter

However, it's not the CD20 that is being used for the first multi-crypto ETF. The reason: liquidity.

**CD5** represents the top 5 assets from CD20, but applies final adjustments to ensure maximum stability and liquidity by selecting only assets that consistently demonstrate the deepest, most reliable trading volumes in the crypto market.

The index is much younger, and still lacks equal online coverage, but studying its construction it's clear it was meant to be used by institutions.

Instead of just picking the 5 largest by market cap, it uses **median daily value traded (MDVT)** over 90 days. This means they're not just looking at size—they're looking at **consistent, reliable trading activity**.

Another difference from CD20, the caps are gone, with Bitcoin reaching currently over 80% of the allocation. To me these adjustments are simply a reality check. With the volume of the biggest players, still only Bitcoin is large enough to handle transactions of any size.

## Summary of the Index Structure

To summarize the methodology:

**Index Universe** - establishes baseline eligibility criteria, filters around 50 assets (?). 
**CD20** - applies portfolio management principles including market cap weighting with caps and quarterly rebalancing. 
**CD5** - selects the most liquid assets suitable for institutional investment.

My observation is that while as of 2025, only the smallest index is liquid enough to be used as a foundation for an ETF, it's clear that broader ETFs for the sector are coming.

### Liquidity Adjustments

CD5 is what Grayscale's ETF follows, and if it would use pure market capitalization weighting, here's what the allocation would look like based on current market caps:

- **Bitcoin**: ~73% (theoretical)
- **Ethereum**: ~18% (theoretical) 
- **Solana**: ~4% (theoretical)
- **XRP**: ~4% (theoretical)
- **Cardano**: ~1% (theoretical)

But the actual allocations show a noticeable difference:

- **Bitcoin**: 80.2% (actual)
- **Ethereum**: 11.39% (actual)
- **XRP**: 4.82% (actual)
- **Solana**: 2.78% (actual)
- **Cardano**: 0.81% (actual)

### Further Rebalancing Refinements

Like CD20 described above, CD5 follows the same quarterly rebalancing schedule with buffer rules to prevent excessive turnover. However, since CD5 selects from the already-rebalanced CD20 constituents, its rebalancing focuses specifically on the **median daily value traded (MDVT)** rankings rather than market cap rankings.

This means CD5 constituent changes are driven by liquidity shifts rather than just market cap movements—creating an even more stable index for institutional implementation.

I understand why liquidity matters for institutional investors, but a closer look at the adjustments makes me think: Does the adjustment improve the instrument for retail investors too? Since the index is publicly available, we will obviously offer it on the platform, but further research is necessary.

## Why Institutions Are Paying Attention

The approval signals growing regulatory acceptance of digital assets. For institutional investors—hedge funds, pension funds, and wealth managers—this ETF provides the regulatory comfort they need to allocate capital to crypto.

With Bitcoin recently hitting new highs around $109,000 and the total crypto market cap swelling to approximately $3.5 trillion, the timing of this approval is significant. It opens the door for broader institutional participation in the crypto markets through regulated vehicles.

As crypto analyst [@TeoMercer noted on X](https://x.com/TeoMercer/status/1940101976290623976]):

<blockquote class="twitter-tweet">
<p lang="en" dir="ltr">The SEC has acknowledged Grayscale’s request to convert its Digital Large Cap Fund into a spot ETF.<br>Assets included: <a href="https://twitter.com/search?q=%24BTC&amp;src=ctag&amp;ref_src=twsrc%5Etfw">$BTC</a>, <a href="https://twitter.com/search?q=%24ETH&amp;src=ctag&amp;ref_src=twsrc%5Etfw">$ETH</a>, <a href="https://twitter.com/search?q=%24XRP&amp;src=ctag&amp;ref_src=twsrc%5Etfw">$XRP</a>, <a href="https://twitter.com/search?q=%24SOL&amp;src=ctag&amp;ref_src=twsrc%5Etfw">$SOL</a>, <a href="https://twitter.com/search?q=%24ADA&amp;src=ctag&amp;ref_src=twsrc%5Etfw">$ADA</a>.<br>This move could open the door to broader institutional exposure across top alts.<br>ETF momentum is accelerating—are you positioned for… <a href="https://t.co/CUndPScMWs">pic.twitter.com/CUndPScMWs</a></p>&mdash; ︎ Teo Mercer (@TeoMercer) <a href="https://twitter.com/TeoMercer/status/1940101976290623976?ref_src=twsrc%5Etfw">July 1, 2025</a>
</blockquote>

### The Ripple Effect

ETF Store President [Nate Geraci sees broader implications](https://twitter.com/NateGeraci/status/1939454629915619403), suggesting this approval could pave the way for individual spot ETFs for assets like XRP, Solana, and Litecoin. This would allow investors to gain targeted exposure to specific cryptocurrencies through traditional investment accounts.

<blockquote class="twitter-tweet">
<p lang="en" dir="ltr">Final SEC deadline this week on Grayscale Digital Large Cap ETF (GDLC)…<br><br>Holds btc, eth, xrp, sol, &amp; ada.<br><br>Think *high likelihood* this is approved.<br><br>Would then be followed later by approval for individual spot ETFs on xrp, sol, ada, etc.</p>&mdash; Nate Geraci (@NateGeraci) <a href="https://twitter.com/NateGeraci/status/1939454629915619403?ref_src=twsrc%5Etfw">June 29, 2025</a>
</blockquote>

For assets like XRP and Solana, inclusion in this ETF represents more than just market access—it provides institutional validation and potential price stability through increased institutional demand.

A big moment of triumph for the "XRP army" and surprise for many.

## Looking Ahead 

Each regulatory milestone signals the crypto market's continued maturation. The SEC's approval of diversified crypto ETFs validates digital assets as a legitimate investment class.

This goes beyond just Grayscale's ETF - it marks a fundamental shift in how traditional finance views crypto. As regulated crypto vehicles emerge, market dynamics will evolve to reflect more sophisticated trading patterns.

**The integration of crypto into mainstream finance is inevitable - the only questions are when and how.**

## Our Mission

The launch of this ETF creates an opportunity gap. While institutional investors gain access to sophisticated crypto indexing through regulated ETFs, individual investors face barriers—requiring professional investor status and other extra steps. Almost surely the Grayscale's ETF will be out of reach for most of the readers.

Deltabadger's mission is to make proven portfolio strategies accessible to everyone. Our approach has key advantages:

First, individuals are not constrained by liquidity requirements like ETFs are. This gives us more flexibility in index construction and implementation. Index investing in emerging trends like memecoins while maintaining a professional, passive approach, is within reach.

Second, we're building a powerful indexing engine (launching later this year) that will give users unprecedented control. With access to over 500 indices from CoinGecko and customizable weightings, you'll be able to build portfolios that precisely match your strategy.

This automated approach eliminates the need for manual management and arbitrary decisions. Just as index investing through ETFs and mutual funds has become the standard in traditional markets, we believe automated indexing will become the default for long-term crypto investing.

Indices' biggest advantage is that they provide natural risk management - failed projects drop out automatically, similar to how bankrupt companies exit the S&P 500. And unlike CoinDesk's quarterly rebalancing, custom indices can adapt more quickly as market conditions change.
--

What's your take on these developments? Would you invest in CD5, CD20, or just stick to Bitcoin? ETF or self-custodied custom index portfolio?