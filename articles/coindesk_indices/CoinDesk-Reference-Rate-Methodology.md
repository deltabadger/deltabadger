# CoinDesk Reference Rate Methodology
August 2024

## Introduction

### Objective
The CoinDesk Reference Rates provide real-time, USD-denominated reference rate prices for single digital assets (“Reference Rates”). Each reference rate is calculated using traded prices during a defined lookback period on at least two contributing exchanges. There are two types of reference rates: settlement and spot.

*   **Settlement Reference Rates** provide the volume weighted average price of a digital asset over a one-hour lookback. Contributing exchanges are weighted proportional to their trading volume during the lookback period.
*   **Spot Reference Rates** provide a price that is more reflective of the current spot price of a digital asset.
    *   Each Spot Reference Rate is a volume-weighted median price using trades from contributing exchanges over the prior 30-second lookback period.
    *   Trades are binned into ten 3-second buckets
    *   An exponentially weighted algorithm is applied to the resulting bins to produce the reference rate.

### Additional Details
This Reference Rate methodology (“Methodology”) was developed by CoinDesk Indices (“CDI”) to achieve the stated above Objective. There may be circumstances or market events which require CDI, in its sole discretion, to deviate from these rules to ensure each Reference Rate continues to meet its Objective. This document should be read in conjunction with the CoinDesk Digital Asset Indices Policy Methodology.

## Eligibility Criteria

### Digital asset eligibility
Eligible digital assets for which Reference Rates will be constructed and maintained will be determined based on meeting the eligibility criteria defined below.

CoinDesk Indices maintains and calculates multi digital asset index families all of which have their own methodology and eligible digital assets which require reference rates as inputs to the index calculation.

### Exchange eligibility
To be eligible, an exchange must meet the Exchange Eligibility criteria outlined in the CoinDesk Digital Asset Policy Methodology and defined as a Category 1 or Category 2 exchange. For each single asset Reference Rate, the following additional criteria is used to establish eligible exchanges.

1.  Must support USD-denominated trading for the single asset.
2.  USD-denominated trading volume must be greater than zero for each of the 30 consecutive days leading up to and including the reference date. To reduce turnover, the trading volume for an existing contributing exchange must be greater than zero for at least 24 days of the 30-day period leading up to and including the reference date.

For each Reference Rate, the list of exchanges that pass all eligibility criteria listed above form the Selection Universe. The Index Committee reserves the right to relax the eligibility criteria based on market conditions.

## Reference Rate Construction

### Contributing exchange selection
For each digital asset Reference Rate, exchanges in the Selection Universe are included subject to a minimum of two and maximum of three contributing exchanges (“Contributing Exchanges”).[^1] Contributing Exchanges for each digital asset Reference Rate are determined each quarter based on the following process:

**Step 1:** Determine the 90-day notional volume in the USD-quoted market on each Eligible Exchange. If less than 90 days are available, all available data is used to determine the 90-day notional volume.

**Step 2:** Rank the results of Step 1 in descending order.

**Step 3:** Select the two highest ranked Category 1 Exchanges (U.S. Licensed exchanges) from Step 2. See the CoinDesk Digital Asset Indices Policy Methodology for a list of Category 1 Exchanges.

**Step 4:** If additional Category 1 or Category 2 exchanges remain after Step 3, the third Contributing Exchange will be selected from the remaining exchanges based on the following rule: If one of the remaining exchanges is an existing Contributing Exchange, it will remain a Contributing Exchange unless the highest ranked non-contributing exchange not selected in Step 3 is 1.20 times the volume of the current Contributing Exchange. Otherwise, the third exchange selected will be the highest ranked exchange not selected in Step 3.

## Reference Rate Calculation

### Settlement Reference Rates
Settlement Reference Rates are calculated using a volume weighted average price (VWAP) across Contributing Exchanges over the prior 60 minutes. Price and volume inputs are sourced and normalized from the Contributing Exchanges. Additional validations on the data inputs, such as outliers, are not performed.

#### Settlement Reference Rate Calculation Formula
\[
\text{Settlement Reference Rate}_{i,t} = \frac{\sum (\text{TradeVolume}_{i,t,LB} \times \text{TradePrice}_{i,t,LB})}{\sum \text{TradeVolume}_{i,t,LB}}
\]

where,

-   _`Settlement Reference Rate`<sub>i,t</sub>_ = Settlement Reference Rate of digital asset i at time t
-   _`TradeVolume`<sub>i,t,LB</sub>_ = Volume of each trade of digital asset i during the lookback period
-   _`TradePrice`<sub>i,t,LB</sub>_ = Price of each trade of digital asset i during the lookback period
-   _`n`_ = number of constituent exchanges
-   _`LB`_ = lookback period of 60 minutes from time t

### Spot Reference Rates
Spot Reference Rates are an exponentially weighted average of time-based intervals of trades from constituent exchanges. They include trades from the 30-second period leading up to the time 𝑡 grouped into 10 3-second intervals (“bins”).

#### Spot Reference Rate Calculation Formula
\[
\text{Spot Reference Rate}_{i,t} = \sum_{bin=1}^{10} w_{bin} \times VWMP_{i,t,bin}
\]

where,

-   _`Spot Reference Rate`<sub>i,t</sub>_ is the Spot Reference Rate of digital asset i at time t
-   _`w`<sub>bin</sub>_ is the weight of each bin
-   _`VWMP`<sub>i,t,bin</sub>_ is the Volume-Weighted Median price of digital asset i at time t for bin 𝑏𝑖𝑛

The weights of each bin are determined by an exponential function where the most recent bin receives the greatest weight and the furthest back receives the least weight.

| Bin | Weight      |
| --- | ----------- |
| 1   | 22.902126%  |
| 2   | 18.177430%  |
| 3   | 14.427435%  |
| 4   | 11.451063%  |
| 5   | 9.088715%   |
| 6   | 7.213718%   |
| 7   | 5.725532%   |
| 8   | 4.544357%   |
| 9   | 3.606859%   |
| 10  | 2.862766%   |

The volume-weighted median price (VWMP) of digital asset i at time t for bin 𝑏𝑖𝑛 is determined by the following algorithm:

1.  Sort trades by their corresponding prices, where each trade 𝑘 is ordered by its price (𝑃<sub>𝑘</sub>)
2.  For each trade 𝑘 calculate the cumulative volume within the bin:
    \[ \text{Cumulative Volume}_k = \sum_{j=1}^{k} V_j \]
3.  Determine the total volume of all 𝑁 trades in the bin:
    \[ \text{Total Volume} = \sum_{k=1}^{N} V_k \]
4.  Identify the minimum price of the trade where its 𝐶𝑢𝑚𝑢𝑙𝑎𝑡𝑖𝑣𝑒 𝑉𝑜𝑙𝑢𝑚𝑒 is greater than or equal to 1/2 of the 𝑇𝑜𝑡𝑎𝑙 𝑉𝑜𝑙𝑢𝑚𝑒 of the bin:
    \[ \text{VWMP} = \min(P_k) \quad \text{where} \quad \text{Cumulative Volume}_k \ge \frac{\text{Total Volume}}{2} \]

### Empty bins are handled using the following:
*   If a given bin contains no trades, the empty bin is filled with the price of the previous bin.
*   If there are no previous non-empty bins within the 30-second window, then the bin is left empty, and the weights of non-empty bins are divided by their total so that they sum to 100%.

### Calculation frequency
Settlement and Spot Reference Rates for digital asset tokens are calculated approximately every five seconds.

### Inactivity during lookback period for Spot and Settlement Reference Rates
There may be periods of trading inactivity on Contributing Exchanges. A digital asset’s Reference Rate will continue to be calculated provided at least one Contributing Exchange has an eligible trade during the applicable lookback period. In the event there are no trades on any of the Contributing Exchanges during the applicable lookback period, the most recent calculated Reference Rate price will be maintained until trading resumes. Contributing Exchanges that do not meet the minimum trading requirements defined in Eligibility Criteria will be removed at the next reconstitution.

## Reference Rate Maintenance
The review process and schedule for the Settlement and Spot Reference Rates are identical and are detailed below.

### Quarterly Review
Each CoinDesk Reference Rate is reviewed quarterly based on the rules described above. Any constituent exchange changes resulting from the quarterly review are announced two weeks prior to the effective date and implemented on the second business day of the reconstitution months: January, April, July, and October. Please see Table 1 for the reconstitution timings.

**Table 1: Reconstitution Timing**
| Activity Description          | Timing                                                                                | Example Reconstitution Timing |
| ----------------------------- | ------------------------------------------------------------------------------------- | ----------------------------- |
| Reconstitution Reference Date | Announcement Date minus 2 business days                                               | 06/14/2024                    |
| Announcement Date             | Effective Date minus 14 calendar days (following business day if this is a holiday)   | 06/18/2024                    |
| Effective Date                | 4PM on the 2nd business day of the start of each quarter                              | 07/02/2024                    |

In addition to the quarterly process, each Reference Rate is monitored for trading disruptions. Out-of-review monitoring, which would require a modification, only applies in extraordinary circumstances.

### Deletions
If a digital asset is delisted or no longer trading on a Contributing Exchange, the impacted exchange will no longer contribute to the calculation of the applicable Reference Rate (see Additions section).

### Additions
There will be no additions to the Reference Rates between reconstitutions unless the number of exchanges contributing to the calculation of the Reference Rate drops below 2. If the constituent count drops below 2, the Index Committee will review the eligible universe and determine the appropriate action. The action may include adding an additional exchange prior to the next scheduled Reconstitution. In certain circumstances, a Reference Rate may be calculated with only one contributing exchange.

### Reference rate termination
CDI reserves the right to terminate a Reference Rate for a digital asset, including in the event the required number of Contributing Exchanges cannot be met.

## Data Distribution
Real-time reference rates values are calculated 24x7 at 5 second intervals and are available to subscribers via API and scheduled email updates.

## Reference Rate Governance
The CoinDesk Index Committee (“Index Committee”) provides ongoing governance of each Reference Rate and this Methodology. For more details on the Index Committee, please refer to the Index Governance section of the CoinDesk Digital Asset Indices Policy Methodology.

## Appendix 1: Data sources
This section describes the data sources used to maintain, reconstitute and calculate the Indices. If data is not available for any reason from the sources described in this appendix, other data sources may be used.

### Price data
Prices used to calculate Reference Rates are sourced from Eligible Exchanges.

### Volume data
Volume data used to calculate Reference Rates are sourced from Eligible Exchanges.

### Reconstitution Data
Daily notional volume is sourced from eligible exchanges. In the event an exchange does not explicitly provide daily notional, CDI will estimate daily notional for a digital asset using the following formula:
\[ \text{Daily Notional Volume} = \text{Daily trading volume} \times \frac{(\text{open price} + \text{close price})}{2} \]

## Appendix 2: Methodology Changes
The table below is a summary of modifications to this Methodology.

| Effective Date | Material Change | Prior Treatment                                                                                                             | Updated Treatment                                                                                             |
| :------------- | :-------------- | :-------------------------------------------------------------------------------------------------------------------------- | :------------------------------------------------------------------------------------------------------------ |
| 5/1/2024       | Yes             | Contributing Exchanges for each CoinDesk Reference Rate reviewed monthly.                                                   | Contributing Exchanges for each CoinDesk Reference Rate reviewed quarterly.                                   |
| 4/1/2024       | Yes             | Only U.S. licensed exchanges are eligible for inclusion.                                                                    | U.S. and non-U.S. exchanges that meet eligibility requirements are eligible for inclusion subject to a minimum of 2 U.S. Licensed exchanges |
| 8/23/2023      | No              | Reference rates with three constituent exchanges were not updated on non-quarter-end reviews unless a contributing exchange failed the exchange eligibility criteria | All reference rates undergo a full review of each month. Any updates are announced and implemented based on the reconstitution timing |

## Appendix 3: Document Revision History

| Timing      | Description                                                                                                                                                    |
| :---------- | :------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 08/23/2024  | Addition of Spot Reference Rates                                                                                                                               |
| 4/29/2024   | See Methodology Changes appendix.                                                                                                                              |
| 4/1/2024    | See Methodology Changes appendix.                                                                                                                              |
| 8/23/2023   | Annual methodology performed including minor updates and clarifications and the modification included in the Methodology Changes appendix.                     |
| 07/21/2023  | Updates and clarifications to various sections.                                                                                                                |
| 2/16/2023   | Inserted <<Monthly Review>> description under Reference Rate Maintenance section to provide additional clarification on potential additions/deletions of contributing exchanges between quarterly reviews. |
| 12/21/2022  | Clarification in Data Sources appendix related to the procedure to estimate notional volume.                                                                   |
| 10/9/2022   | Minor edits                                                                                                                                                    |
| 8/23/2022   | Initial Version                                                                                                                             