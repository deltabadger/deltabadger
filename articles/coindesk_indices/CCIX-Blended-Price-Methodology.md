# CCData Blended Prices
_CC Data Limited_
_January 2025_

## Introduction

CCData Blended Prices (CCIXB) are digital asset reference rates that combine
fiat and converted stablecoin pair traded prices. The methodology is calculated
as a 24-hour volume-weighted average price using a customised selection of ex-
changes and an outlier detection methodology, converting the final prices into
USD.

## CCData Blended Prices

Currently, the following CCData Blended Prices are available:

-   **CCData Blended Exchange Variant Prices**
    The CCData Blended Exchange Variant Prices (CCIXBE) blend USD,
    USDT and USDC trades and return a final USD price.
-   **CCData Blended Exchange Variant Reduced Universe Prices**
    The CCData Blended Exchange Variant Reduced Universe Prices (CCIXBER)
    blend USD and USDC trades and return a final USD price.

# Definitions

-   **24 Hour Volume** means, with respect to an Underlying Pair or Stablecoin-
    USD Pair, an Exchange and a point in time, the sum of the volume of
    such Underlying Pair or Stablecoin-USD Pair on such Exchange over the
    last 23 calendar hours and the cumulative volume of the current calendar
    hour.
-   **Blended Price** means, with respect to a digital asset, a volume-weighted
    real-time reference rate for such digital asset that blends USD and certain
    stablecoin markets. It is calculated in accordance with Equation 1.
-   **Exchange** means, with respect to a Blended Price, an exchange that
    trades cryptocurrencies and is selected to contribute to such rate.
-   **FX Conversion Rate** means, with respect to an Underlying Pair, a
    volume-weighted real-time FX conversion rate for the relevant Stablecoin-
    USD Pair. It is calculated in accordance with Equation 2.
-   **Outlier Detection Factor** means a factor used for penalising a price
    deemed to be an outlier in the Blended Price or FX Conversion Rate
    calculation and is determined in accordance with Equation 4.
-   **Stablecoin-USD Pair** means, with respect to an Underlying Pair, the
    relevant stablecoin against USD.
-   **Time Penalty Factor** means a factor used for penalising outdated prices
    in the Blended Price or FX Conversion Rate calculation and is determined
    in accordance with Equation 7.
-   **Underlying Pair** means, with respect to a Blended Price, the USD mar-
    ket and each stablecoin market for the underlying digital asset that con-
    stitutes the Blended Price.
-   **UTC** stands for Coordinated Universal Time.

## Price Calculation

### CCData Blended Prices Calculation

The CCData Blended Prices (CCIXB) follow the spot market performance
and are calculated every time a new trade is received on one of the con-
stituent Exchanges and disseminated every 5-seconds. The methodology
blends fiat currencies and stable coins and returns a final USD price. Prices
are calculated according to the following formula:

$$
P_t = \sum_{u \in U_t} \sum_{e \in E_t^u} w_t^{u,e} \cdot p_t^{u,e} \cdot FX_t^u \quad (1)
$$

Where:

-   `t` denotes a point in time, where the integer value represents seconds in unix timestamps[^1];
-   `P_t` is the CCIXB value at time t;
-   `u` denotes an Underlying Pair in set `U_t`;
-   `U_t` is, with respect to CCIXB, the set of all Underlying Pairs;
-   `e` denotes an Exchange in set `E_t^u`;
-   `E_t^u` is, with respect to Underlying Pair u, the set of all constituent Exchanges;
-   `w_t^{u,e}` is, with respect to Underlying Pair u, the weight assigned to Exchange e at time t and is calculated in accordance with Equation 3
-   `p_t^{u,e}` is, with respect to Underlying Pair u, Exchange e and time t, the price of the last trade on such Exchange e; and
-   `FX_t^u` is, with respect to Underlying Pair u, the FX Conversion Rate value at time t for the relevant Stablecoin-USD Pair calculated in accordance with Equation 2.

#### FX Conversion Rate Calculation

With respect to each Underlying Pair, the FX Conversion Rate value is the price of the underlying stablecoin in USD. For example, for Underlying Pair BTC-USDT, the relevant FX Conversion Rate value is the price of USDT in USD. For the avoidance of doubt, if the quote currency of the Underlying Pair is USD, then an FX Conversion Rate value is not applicable. The rate is calculated real-time in the same manner as the Blended Price described in [CCData Blended Prices Calculation](#ccdata-blended-prices-calculation) but with the following changes:

-   `FX_t^u` is now defined as the final rate value at time `t` and omitted from the rest of the equation;
-   `U_t` is defined as the unit set of the relevant Stablecoin-USD Pair. For example, for Underlying Pair BTC-USDT, the set `U_t` would be the unit set containing USDT-USD.

By applying these changes, the FX Conversion Rate formula for a given Underlying Pair u can be simplified as follows for the case when we have a direct pair:

$$
FX_t^u = \sum_{e \in E_t^s} w_t^{s,e} \cdot p_t^{s,e} \quad (2)
$$

Where:

-   `t` denotes a point in time, where the integer value represents seconds in unix timestamps;
-   `FX_t^u` is, with respect to Underlying Pair u, the FX Conversion Rate value at time t;
-   `s` is, with respect to Underlying Pair u, the relevant Stablecoin-USD Pair;
-   `e` denotes an Exchange in set `E_t^s`;
-   `E_t^s` is, with respect to Stablecoin-USD Pair s, the set of all Stablecoin-USD Pair constituent Exchanges;
-   `w_t^{s,e}` is, with respect to Stablecoin-USD Pair s, the weight assigned to Exchange e at time t and is calculated in accordance with Equation 3; and
-   `p_t^{s,e}` is, with respect to Stablecoin-USD Pair s, Exchange e and time t, the price of the last trade on such Exchange e.

#### Exchange Weight Calculation

With respect to Underlying Pair u or Stablecoin-USD Pair s, the weight of Exchange e at time t is calculated as follows:[^2]

$$
w_t^{u,e} = \frac{\Pi_t^{u,e} \cdot V_t^{u,e} \cdot \gamma_t^{u,e}}{\sum_{x \in E_t^u} \Pi_t^{u,x} \cdot V_t^{u,x} \cdot \gamma_t^{u,x}} \quad (3)
$$

Where:

-   `x` denotes an Exchange (including Exchange e) in set `E_t^u`;
-   `\Pi_t^{u,e}` is, with respect to Underlying Pair u, Exchange e and time t, the Outlier Detection Factor determined in accordance with Equation 4;
-   `V_t^{u,e}` is, with respect to Underlying Pair u, Exchange e and time t, the 24 Hour Volume calculated in accordance with Equation 5; and
-   `\gamma_t^{u,e}` is, with respect to Underlying Pair u, Exchange e and time t, the Time Penalty Factor determined in accordance with Equation 7.

The Outlier Detection Factor, with respect to Underlying Pair u, Exchange e and time t, is determined as follows:

$$
\Pi_t^{u,e} = 
\begin{cases} 
0 & \text{if } |E_t^u| > 2 \text{ and } (p_t^{u,e} > A \cdot P_{l_t} \text{ or } A \cdot p_t^{u,e} < P_{l_t}) \\ 
1 & \text{otherwise} 
\end{cases}
\quad (4)
$$

Where:

-   `E_t^u` and `p_t^{u,e}` are as defined above;
-   `A` is a constant that denotes the price deviation threshold; it is currently set to 1.05;
-   `l_t` is, with respect to t, the time of the last trade from any Exchange to contribute to the Blended Price[^3]; and
-   `P_{l_t}` is the Blended Price value at time `l_t`.

The 24 Hour Volume, as defined in this document, with respect to Underlying Pair u, Exchange e and time t, is calculated as follows:

$$
V_t^{u,e} = \sum_{h_t \le \theta < t} v_\theta^{u,e} \quad (5)
$$

Where:

-   `h_t` is, with respect to time t, the timestamp of the last calendar hour in UTC in the previous 24-hour period determined as follows:
    $$
    h_t = t - (23 \cdot 3600 + c) \quad (6)
    $$
    Where:
    -   `c` is the number of seconds past in the current hour;
-   `\theta` denotes a point in time between `h_t` (inclusive) and `t` (exclusive) for which there was a trade for Underlying Pair u on Exchange e; and
-   `v_\theta^{u,e}` is the quantity traded of Underlying Pair u on Exchange e at time `\theta`.

The Time Penalty Factor, with respect to Underlying Pair u, Exchange e and time t, is determined as follows:

$$
\gamma_t^{u,e} = 
\begin{cases} 
1 & \text{if } \tau_t^{u,e} < 5 \\ 
0.8 & \text{if } 5 \le \tau_t^{u,e} < 10 \\
0.6 & \text{if } 10 \le \tau_t^{u,e} < 15 \\
0.4 & \text{if } 15 \le \tau_t^{u,e} < 20 \\
0.2 & \text{if } 20 \le \tau_t^{u,e} < 25 \\
0.001 & \text{otherwise} 
\end{cases}
\quad (7)
$$

Where:

-   `\tau_t^{u,e}` is, with respect to Underlying Pair u, Exchange e and time t, the length of time in minutes since the last trade on Exchange e calculated as follows:
    $$
    \tau_t^{u,e} = \frac{t - l_t^{u,e}}{60} \quad (8)
    $$
    Where:
    -   `l_t^{u,e}` is, with respect to Underlying Pair u, Exchange e and time t, the time of the last trade on such Exchange e to contribute to the Blended Price[^4].

#### Outlier Detection

Along with the real-time outlier detection dictated by `\Pi_t^{u,e}` in Equation 4, CCData will manually remove trades that are deemed outliers for other reasons, such as exchange errors.

### CCData Blended Prices VWAP Calculation

The CCData Blended Exchange (CCIXB VWAP) is calculated real-time
every hour using the CCIXB prices as input,according to the following
formula:

$$
P_{vwap}(h, m_h) = \frac{\sum_{m_h \in M_h} q(h, m_h) \cdot P(t)}{\sum_t q(t)} \quad (9)
$$

Where:

-   `P_t` is as defined above the CCIXB price at time t
-   `h` is the hour of the day `\in [0,1,...,23]`
-   `M_h` is the total number of ticks in the hour h
-   `q(h,m_h)` describes the CCIXB quantity at the hour of the day h at the m-th tick within the h hour.

Note that the tuple `(h,m_t)` describes a point in time, identical to `t` which allows for the identity of `P(t) = P(h,m_h)`. However, for the sake of clarity and readability, we shall refrain from presenting the price as hour of the day, tick of the hour.

## Constituent Exchange Selection

### CCData Blended Price Exchange Variant Exchange Selection

Exchanges are selected for each Underlying Pair and Stablecoin-USD Pair
in accordance to sections 6 and 7 of the CCIX methodology document.

### CCData Blended Price Exchange Variant Reduced Universe Exchange Selection

Exchanges are selected following the Exchange Eligibility Criteria detailed
in the CoinDesk Digital Asset Indices Policy Methodology.

Exchanges that are designated as ’Excluded Exchanges’ in the CoinDesk
Digital Asset Indices Policy Methodology are excluded from all CCData
Blended Prices calculations.

The constituent Exchanges list can be accessed via the following endpoint:
<https://developers.ccdata.io/documentation/data-api/index_cc_v1_markets_instruments>

## Dissemination

All reference prices are disseminated via REST API and WebSocket API
every 5-seconds. The relevant API endpoints can be found here: <https://developers.ccdata.io/documentation/data-api/index_cc>

[^1]: Therefore 0 represents 00:00:00 on January 1st, 1970 UTC.
[^2]: u may be substituted for s in Equation 3 and all subsequent Equations to calculate the applicable weight for a Stablecoin-USD Pair.
[^3]: In the case of a Stablecoin-USD Pair, `l_t` and `P_{l_t}` would be with respect to the relevant FX Conversion Rate.
[^4]: In the case of a Stablecoin-USD Pair s, `l_t^{u,e}` would be with respect to the relevant FX Conversion Rate.