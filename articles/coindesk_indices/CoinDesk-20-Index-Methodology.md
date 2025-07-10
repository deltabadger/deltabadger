# CoinDesk 20 Index Methodology

_April 2025_

## Table of Contents
- [Introduction](#introduction)
  - [Index Objective](#index-objective)
  - [Additional Details](#additional-details)
  - [Table 1: Index Details](#table-1-index-details)
- [Eligibility Criteria](#eligibility-criteria)
  - [Index Universe Eligibility](#index-universe-eligibility)
- [Index Construction](#index-construction)
  - [Constituent Selection](#constituent-selection)
  - [Constituent Weighting](#constituent-weighting)
- [Index Calculation](#index-calculation)
  - [Constituent Pricing](#constituent-pricing)
  - [Index Calculation Formula](#index-calculation-formula)
  - [Index Settlement Calculation Formula](#index-settlement-calculation-formula)
  - [Index Divisor Adjustment](#index-divisor-adjustment)
- [Index Maintenance](#index-maintenance)
  - [Index Reconstitution](#index-reconstitution)
  - [Table 2: Reconstitution Calendar Example](#table-2-reconstitution-calendar-example)
  - [Index Changes Between Reconstitutions](#index-changes-between-reconstitutions)
- [Data Distribution](#data-distribution)
- [Index Governance](#index-governance)
- [Appendix 1: CoinDesk 20 Index Backtest Methodology](#appendix-1-coindesk-20-index-backtest-methodology)
  - [Overview](#overview)
  - [Eligibility Requirements](#eligibility-requirements)
  - [Selection](#selection)
  - [Table 3: Backtest Specifications at each reconstitution](#table-3-backtest-specifications-at-each-reconstitution)
  - [Constituent Pricing](#constituent-pricing-1)
  - [Circulating Supply](#circulating-supply)
  - [Reconstitution](#reconstitution-1)
  - [Constituent Weighting](#constituent-weighting-1)
  - [Calculation Frequency](#calculation-frequency)
  - [Data Integrity](#data-integrity)
- [Appendix 2: Data Sources](#appendix-2-data-sources)
  - [Reconstitution data](#reconstitution-data)
  - [Pricing Data](#pricing-data)
  - [Custody Data](#custody-data)
- [Appendix 3: Methodology Changes](#appendix-3-methodology-changes)
  - [Table 4: Summary of methodology modifications](#table-4-summary-of-methodology-modifications)


## Introduction
### Index Objective
The CoinDesk 20 Index (“CoinDesk 20,” or “The Index”) measures the performance of the largest twenty digital assets by market capitalization, excluding stablecoins, memecoins, and certain other classifications and that meet certain exchange listing and liquidity requirements [see Index Universe Eligibility]. Constituents are weighted by market capitalization subject to weight caps [see Constituent Weighting].

### Additional Details
This methodology was created and is owned by CoinDesk Indices (“CDI”) to achieve the Index Objective stated above. The Index is administered, calculated and maintained by CDI's affiliate, CC Data Limited ("CCData"), an FCA regulated benchmark administrator. References to CDI in this methodology shall be deemed to include CCData.

There may be circumstances or market events which require CDI, in its sole discretion, to deviate from these rules to ensure each Index continues to meet its Objective. This document should be read in conjunction with the CoinDesk Digital Asset Indices Policy Methodology.

### Table 1: Index Details
| Index Name        | Symbols | Target Count | Launch Date  | Base Value | Base Date   | First Value Date |
| ----------------- | ------- | ------------ | ------------ | ---------- | ----------- | ---------------- |
| CoinDesk 20 Index | CD20    | 20           | Jan 12, 2024 | 1000.00    | Oct 4, 2022 | Dec 29, 2017     |

## Eligibility Criteria
### Index Universe Eligibility
To be included in the Index Universe, a digital asset must meet the following criteria as of the Reconstitution Reference Date:
1.  The digital asset must be:
    a. among the largest 250 digital assets by market capitalization, excluding stablecoins,
    b. able to support an applicable Reference Rate [see Constituent Pricing].
2.  The digital asset must not be:
    a. a wrapped, pegged, or staked asset or a gas token,
    b. a memecoin or a privacy token,
    c. an asset that meets the definition of a security as defined in the Policy Methodology.
3.  The digital asset must be listed as a USD and/or USDC pair on a minimum of three exchanges that contribute to the applicable Reference Rate [see Constituent Pricing] and meet the following requirements:
    a. at least one listing has existed for the previous 90 days,
    b. at least one listing is available to U.S. customers,
    c. there has been trade volume in each contributing pair in each of the previous 30 days on three or more contributing exchanges.

The Index Committee reserves the right to relax the eligibility criteria if an insufficient number of digital assets qualify.

## Index Construction
### Constituent Selection
The constituent selection process targets the 20 largest digital assets by market capitalization that are in the Index Universe and meet certain trading and liquidity requirements. The selection process relaxes certain requirements for current constituents to reduce excess turnover.

Constituents are selected from the Index Universe using the following steps:
1.  Rank all digital assets in the Index Universe by 90-day median daily value traded (MDVT) in descending order up to and including the Reconstitution Reference Date. Daily volume data is sourced from USD and USDC trading pairs aggregated across centralized digital asset exchanges that contribute to the applicable Reference Rate [see Constituent Pricing].
2.  Select the 40 (50) highest ranked non-constituents (current constituents) from the results of Step 1.
3.  Remove all digital assets from Step 2 that are not supported by Coinbase Custody Trust Company.
4.  Rank the results of Step 3 by market capitalization in descending order.
5.  Select the top 15 ranked digital assets as constituents from the previous step.
6.  From the remaining digital assets not selected in the previous step, select current constituents ranked by market capitalization within the top 25 until 20 constituents are selected.
7.  If the previous step results in fewer than 20 constituents, select the highest ranked non-constituents from the remaining digital assets not selected in the previous step until 20 constituents are selected.

### Constituent Weighting
Constituents are weighted by market capitalization subject to the following capping requirements imposed at each reconstitution:
1.  Determine the uncapped market capitalization weight of each constituent using pricing determined by the applicable Settlement Reference Rates.
2.  If the weight of the largest uncapped constituent exceeds 30%, reduce its weight to 30% and redistribute the excess weight to all uncapped constituents in proportion to their market capitalization.
3.  If any remaining constituent’s weight exceeds 20%, reduce its weight to 20% and redistribute the excess weight to all uncapped constituents in proportion to their market capitalization.
4.  Repeat the previous step until all remaining constituents’ weights do not exceed 20%.

Weighting Adjustment Factors are determined on the Weighting Reference Date [see Index Maintenance below] to achieve the weights calculated in the process described above, locking in the circulating supply values used to calculate market capitalization. As such, constituent weights as of the Effective Date will drift with constituent price movements between the Weighting Reference Date and the Effective Date.

## Index Calculation
CoinDesk 20 is calculated in real time using the applicable Reference Rates for underlying constituents.

### Constituent Pricing
Constituent prices are calculated using CCData Blended Prices (“Reference Rates”), configured to include the following requirements:
1.  Exchange eligibility
    a. Adherence to the Policy Methodology exchange requirements.
    b. A minimum rating of BB in CCData’s Exchange Benchmark Report. For more information on the Exchange Benchmark, visit [here](https://ccdata.io/research/exchange-benchmark-rankings).
    c. Not classified as an Excluded Exchange as defined in the Policy Methodology.
2.  Applicable pairs: USD and USDC pairs are included. USDC pairs are converted to USD using the applicable USDC/USD Reference Rate.

CCData Blended Prices VWAP (“Settlement Reference Rates”), are used on the Weighting Reference Date to calculate constituents weights [see Constituent Weighting], to calculate Index Settlement Values [see Index Settlement Calculation Formula] and on the Effective Date to calculate the Index Divisor [see Index Divisor Adjustment]. The Settlement Reference Rates for the Index use the same configuration as the Reference Rates described above.

The methodology for the CCData Blended Reference Prices, referred to as CCIXBER, can be found [here](https://ccdata.io/indices/regulatory).

### Index Calculation Formula
The Index is calculated using the following formula:
$$
Index_{t} = \frac{\sum_{i=1}^{N}P_{i,t} \times S_{i} \times WAF_{i}}{Divisor}
$$
where,
- **Index~t~** is the value of the Index at time t
- **P~i,t~** is the price of constituent i at time t, as determined by its Reference Rate,
- **S~i~** is the circulating supply of constituent i as of the Weighting Reference Date,
- **WAF~i~** is the Weighting Adjustment Factor of constituent i, as of the Weighting Reference Date,
- **Divisor** is the Index Divisor.

### Index Settlement Calculation Formula
A settlement value for the Index will be calculated daily, using prices determined by the applicable Settlement Reference Rates, using the following formula:
$$
Index_{SETT} = \frac{\sum_{i=1}^{N}P_{i,SETT} \times S_{i} \times WAF_{i}}{Divisor}
$$
where,
- **Index~SETT~** is the settlement value of the Index,
- **P~i,SETT~** is the price of constituent i as determined by its Settlement Reference Rate.

### Index Divisor Adjustment
The Index Divisor is recalculated on each reconstitution Effective Date and during any event which requires a change to the index constituents not driven solely by market price movements.
$$
Divisor_{NC} = Divisor_{PC} \times \frac{\sum_{i=1}^{N}P_{i,SETT} \times S_{NC} \times WAF_{NC}}{\sum_{i=1}^{N}P_{i,SETT} \times S_{PC} \times WAF_{PC}}
$$
where,
- **P~i,SETT~** is the price of constituent i as of the Effective Date, or other date and time at which the Divisor Adjustment takes place, determined by its Settlement Reference Rate,
- The subscript **PC** represents the respective values of the prior index constituents,
- The subscript **NC** represents the respective values of the new index constituents following the application of all reconstitution changes.

## Index Maintenance
### Index Reconstitution
The Index is reviewed and reconstituted on a quarterly basis based on the rules described above [see Index Construction]. Any index changes resulting from the quarterly review are announced two weeks prior to the Effective Date, at which time the changes are implemented. Settlement Reference Rates, as defined above [see Constituent Pricing] are used for implementation on the Effective date. Reconstitutions are scheduled so that the Effective Dates fall on the last business day¹ of January, April, July and October. Reconstitutions include four events, defined as follows:

1.  **Reconstitution Reference Date.** Snapshot date for data used to select Index constituents. This falls two business days before the Announcement Date.
2.  **Announcement Date.** The date on which changes to Index constituents are announced. This falls fourteen calendar days before the Effective Date, or the closest following business day.
3.  **Weighting Reference Date.** The date on which the Index weights and Weighting Adjustment Factors are calculated, as defined above [see Index Construction]. This falls seven calendar days before the Effective Date, or the closest following business day.
4.  **Effective Date.** 4 p.m. Eastern Time on the date on which the reconstitution becomes effective.

### Table 2: Reconstitution Calendar Example
| Activity                   | Timing                                                                       | Example      |
| -------------------------- | ---------------------------------------------------------------------------- | ------------ |
| Reconstitution Reference Date | 2 business days prior to the Announcement Date                            | Dec 31, 2024 |
| Announcement Date          | 4 weeks prior to the Effective Date, or if not a business day, the following business day | Jan 3, 2025  |
| Weighting Reference Date   | 7 calendar days prior to the Effective Date, or, if not a business day, the following business day | Jan 24, 2025 |
| Effective Date             | 4 p.m. Eastern Time on the final business day of January, April, July, and October | Jan 31, 2025 |

### Index Changes Between Reconstitutions
In addition to the quarterly process, constituents are monitored for potential anomalies and trading disruptions. Out-of-review monitoring, which would require an index modification, only applies in extraordinary circumstances. Incident types that would require one or more index modifications are outlined in the Policy Methodology.

---
¹ Business days are defined in the Policy Methodology.

### Additions
There will be no additions to the index between reconstitutions.

### Deletions
If a constituent is removed from The Index outside of the scheduled reconstitution process it will not be replaced and, therefore, the constituent count may drop below the target number. The weight of the constituent being removed will be redistributed proportionally to all remaining constituents as of the effective date of removal. No recapping will be performed. The impacted constituent will be considered for inclusion at future reconstitutions if it meets the criteria described above.

## Data Distribution
Index values are calculated 24x7 and are available publicly at coindesk.com and are also available to subscribers via REST and WebSocket APIs.

## Index Governance
Pursuant to CDI’s arrangement with its affiliate CCData to perform administration and calculation services, the Indices are subject to CCData’s governance and oversight functions. For more details on CCData, see [here](https://ccdata.io/indices/regulatory). These provisions override the governance and oversight provisions in the Policy methodology.

## Appendix 1: CoinDesk 20 Index Backtest Methodology
### Overview
In September 2024, CDI completed a backtest of the CoinDesk 20 Index to extend history prior to the base date of October 4, 2022. History was extended back to December 29, 2017. This backtest of the CoinDesk 20 Index sourced data and constituents from the CoinDesk Market Index (“CMI”). The CMI constituents were evaluated according to their methodology beginning August 29, 2022. Prior to that date, CMI constituents were carried backward based on the data availability of the constituent list as of August 29, 2022. Details of the CMI backcast are included in the CoinDesk Market Index Methodology found [here](https://downloads.coindesk.com/cd3/CDI/CoinDesk-Digital-Asset-Indices-Policy-Methodology.pdf).

### Eligibility Requirements
To be eligible for the backtest, a digital asset must have been a constituent of the CoinDesk Market Index prior to the reconstitution effective date. Stablecoins are excluded from the CoinDesk Market Index. Meme coins and privacy tokens were eligible for inclusion.

### Selection
Digital assets that were included in the CoinDesk Market Index at the prior monthly CMI reconstitution are eligible for selection. Constituents are selected at each reconstitution based on market capitalization on the reconstitution effective date. The selection process targets a constituent count (“Target Count”) and incorporates upper and lower buffers to minimize turnover. Below are the steps to select constituents:

-   **Step 1:** Rank all CMI constituents based on market capitalization in descending order.
-   **Step 2:** Select all digital assets from Step 1 that are within the upper buffer
-   **Step 3:** Select all current constituents not selected in Step 2 that are within the lower buffer up to the Target Count.
-   **Step 4:** If the Target Count is not achieved in Step 3, select the top ranked non-constituent not selected in Step 2 until the Target Count is achieved.

The Target Count included at each reconstitution was reduced in the earlier backtest periods to reflect the smaller universe of eligible constituents included in the Market Index. See the following table for the Target Counts and upper and lower buffers applied during the backtest period.

### Table 3: Backtest Specifications at each reconstitution
| Reconstitution Effective Date | Target Count | Upper Buffer | Lower Buffer | Cap % Largest | Cap % All Other |
| ----------------------------- | ------------ | ------------ | ------------ | ------------- | --------------- |
| 12/29/17                      | 5            | 4            | 6            | 50%           | 25%             |
| 4/3/18                        | 5            | 4            | 6            | 50%           | 25%             |
| 7/3/18                        | 5            | 4            | 6            | 50%           | 25%             |
| 10/2/18                       | 5            | 4            | 6            | 50%           | 25%             |
| 1/3/19                        | 5            | 4            | 6            | 50%           | 25%             |
| 4/2/19                        | 5            | 4            | 6            | 50%           | 25%             |
| 7/2/19                        | 5            | 4            | 6            | 50%           | 25%             |
| 10/2/19                       | 5            | 4            | 6            | 50%           | 25%             |
| 1/3/20                        | 10           | 8            | 12           | 40%           | 20%             |
| 4/2/20                        | 10           | 8            | 12           | 40%           | 20%             |
| 7/2/20                        | 10           | 8            | 12           | 40%           | 20%             |
| 10/2/20                       | 10           | 8            | 12           | 40%           | 20%             |
| 1/5/21                        | 20           | 15           | 25           | 30%           | 20%             |
| 4/2/21                        | 20           | 15           | 25           | 30%           | 20%             |
| 7/2/21                        | 20           | 15           | 25           | 30%           | 20%             |
| 10/4/21                       | 20           | 15           | 25           | 30%           | 20%             |
| 1/4/22                        | 20           | 15           | 25           | 30%           | 20%             |
| 4/4/22                        | 20           | 15           | 25           | 30%           | 20%             |
| 7/5/22                        | 20           | 15           | 25           | 30%           | 20%             |

### Constituent Pricing
Digital asset pricing was sourced from Amberdata’s historical spot price pairs endpoint. For more information on Amberdata’s pricing methodology, please visit https://www.amberdata.io/.

### Circulating Supply
Circulating Supply was sourced from Amberdata’s historical supply endpoint and updated monthly. Supplies were pulled from the last calendar day of each month and implemented in the following month’s reconstitution. For more information on Amberdata’s circulating supply methodology, please visit https://www.amberdata.io/.

### Reconstitution
The CoinDesk 20 Index was reconstituted quarterly and implemented at 4p.m. Eastern Time on the 2nd business day of January, April, July, and October. The reference date for data is the reconstitution effective date.

### Constituent Weighting
Constituents are weighted by market capitalization subject to capping requirements imposed at each reconstitution. Capping is applied based on constituent prices on the reconstitution effective date. Capping percentages are based on the Target Count with higher capping percentages applied when the Target Count was below 20. See Table 3 for the capping percentages applied at each reconstitution.

### Calculation Frequency
Index levels during the backcast were calculated on a daily basis at 4pm Eastern Time.

### Data Integrity
Historical supply and pricing data from Amberdata were reviewed by CDI’s research and operations teams to identify and resolve gaps or inconsistencies in circulating supply and price data.

## Appendix 2: Data Sources
This section describes data sources used to maintain, reconstitute, and calculate the Indices since the initial base date. If data are not available for any reason from the sources described in this appendix, other data sources may be used.

### Reconstitution data
-   Market capitalization, snapshot pricing and circulating supply data are sourced from CCData at midnight UTC on the Reference Date.
-   Volume data for liquidity analysis is sourced from CoinDesk Data each day of the defined lookback period.

Volume, pricing, classifications and reference data used for reconstitutions are reviewed by CoinDesk Indices analysts for accuracy and reliability. Based on these reviews, CoinDesk Indices reserves the right to update reconstitution data.

### Pricing Data
Pricing for constituents used to calculate the index is sourced from underlying exchanges and trading venues.

### Custody Data
Custody for digital assets is source from Coinbase Custody Trust Company

## Appendix 3: Methodology Changes
The table below is a summary of modifications to this Methodology.

### Table 4: Summary of methodology modifications
| Effective Date | Prior Treatment | Updated Treatment | Material Change |
| :--- | :--- | :--- | :--- |
| Apr 2025 Reconstitution | No custody requirement | Digital asset must be supported by Coinbase Custody Trust Company | Yes |
| Feb 11, 2025 | Reference Rates based on USD-Denominated paris and calculated using an exponentially weighted 30 second lookback period | Reference Rates based on USD- and USDC-Denominated pairs and calculated using CCIX’s Blended pricing methodology and use Eligible Exchanges that received a rating of BB or higher within CCData’s Exchange Benchmark | Yes |
| Jan 2025 Reconstitution | Index Universe sourced from the CoinDesk Market Index | Index Universe sourced from the top 250 digital assets by market capitalization, excluding stablecoins | Yes |
| Dec 18, 2024 | Reconstitution implemented on the 2nd business day of Jan/Apr/Jul/Oct and announced 2 weeks prior | Reconstitution implemented on the last business day of Jan/Apr/Jul/Oct and announced 4 weeks prior | Yes |
| Dec 10, 2024 | Index calculation formula is a weighted return of each constituent’s weight as of the prior reconstitution effective date multiplied by its price return since the prior reconstitution effective date. | Index is calculated by summing up the market value of each constituent and dividing by a Divisor | No |
| Aug 27, 2024 | Only CD20 Index available | CD20SPOT Index added as an alternate calculation of CD20 | No |
| July 2024 Reconstitution | Meme coins and privacy tokens eligible for inclusion | Meme coins and privacy tokens are not eligible for inclusion | Yes |

