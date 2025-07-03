CoinDesk 20 Index

The CoinDesk 20 Index ("CoinDesk 20," or "The Index") measures the performance of the largest twenty digital assets by market capitalization, excluding stablecoins, memecoins, and certain other classifications and that meet certain exchange listing and liquidity requirements [see Index Universe Eligibility]. Constituents are weighted by market capitalization subject to weight caps [see Constituent Weighting].

# Additional Details

This methodology was created and is owned by CoinDesk Indices ("CDI") to achieve the Index Objective stated above. The Index is administered, calculated and maintained by CDI's affiliate, CC Data Limited ("CCData"), an FCA regulated benchmark administrator. References to CDI in this methodology shall be deemed to include CCData.

There may be circumstances or market events which require CDI, in its sole discretion, to deviate from these rules to ensure each Index continues to meet its Objective. This document should be read in conjunction with the CoinDesk Digital Asset Indices Policy Methodology.

## Table 1: Index Details

| Index Name | Symbols | Target Count | Launch Date | Base Value | Base Date | First Value Date |
|------------|---------|--------------|-------------|------------|-----------|------------------|
| CoinDesk 20 Index | CD20 | 20 | Jan 12, 2024 | 1000.00 | Oct 4, 2022 | Dec 29, 2017 |

# Eligibility Criteria

## Index Universe Eligibility

To be included in the Index Universe, a digital asset must meet the following criteria as of the Reconstitution Reference Date:

1. The digital asset must be:
   - among the largest 250 digital assets by market capitalization, excluding stablecoins
   - able to support an applicable Reference Rate [see Constituent Pricing]

2. The digital asset must not be:
   - a wrapped, pegged, or staked asset or a gas token
   - a memecoin or a privacy token
   - an asset that meets the definition of a security as defined in the Policy Methodology

3. The digital asset must be listed as a USD and/or USDC pair on a minimum of three exchanges that contribute to the applicable Reference Rate [see Constituent Pricing] and meet the following requirements:
   - at least one listing has existed for the previous 90 days
   - at least one listing is available to U.S. customers
   - there has been trade volume in each contributing pair in each of the previous 30 days on three or more contributing exchanges

The Index Committee reserves the right to relax the eligibility criteria if an insufficient number of digital assets qualify.

# Index Construction

## Constituent Selection

The constituent selection process targets the 20 largest digital assets by market capitalization that are in the Index Universe and meet certain trading and liquidity requirements. The selection process relaxes certain requirements for current constituents to reduce excess turnover.

**Buffer Rules**: To prevent excessive turnover, the index uses a "buffer rule" system that gives current constituents preference to remain in the index. New assets must rank higher (top 40) to be considered for inclusion, while existing constituents can rank lower (up to 50) and still qualify. This prevents the "revolving door" effect where assets hovering around the cutoff line are constantly added and removed each quarter.

Constituents are selected from the Index Universe using the following steps:

1. Rank all digital assets in the Index Universe by 90-day median daily value traded (MDVT) in descending order up to and including the Reconstitution Reference Date. Daily volume data is sourced from USD and USDC trading pairs aggregated across centralized digital asset exchanges that contribute to the applicable Reference Rate [see Constituent Pricing].

2. Select the 40 (50) highest ranked non-constituents (current constituents) from the results of Step 1.

3. Remove all digital assets from Step 2 that are not supported by Coinbase Custody Trust Company.

4. Rank the results of Step 3 by market capitalization in descending order.

5. Select the top 15 ranked digital assets as constituents from the previous step.

6. From the remaining digital assets not selected in the previous step, select current constituents ranked by market capitalization within the top 25 until 20 constituents are selected.

7. If the previous step results in fewer than 20 constituents, select the highest ranked non-constituents from the remaining digital assets not selected in the previous step until 20 constituents are selected.

## Constituent Weighting

Constituents are weighted by market capitalization subject to the following capping requirements imposed at each reconstitution:

1. Determine the uncapped market capitalization weight of each constituent using pricing determined by the applicable Settlement Reference Rates.

2. If the weight of the largest uncapped constituent exceeds 30%, reduce its weight to 30% and redistribute the excess weight to all uncapped constituents in proportion to their market capitalization.

3. If any remaining constituent's weight exceeds 20%, reduce its weight to 20% and redistribute the excess weight to all uncapped constituents in proportion to their market capitalization.

4. Repeat the previous step until all remaining constituents' weights do not exceed 20%.

Weighting Adjustment Factors are determined on the Weighting Reference Date [see Index Maintenance below] to achieve the weights calculated in the process described above, locking in the circulating supply values used to calculate market capitalization. As such, constituent weights as of the Effective Date will drift with constituent price movements between the Weighting Reference Date and the Effective Date.

# Index Calculation

CoinDesk 20 is calculated in real time using the applicable Reference Rates for underlying constituents.

## Constituent Pricing

Constituent prices are calculated using CCData Blended Prices ("Reference Rates"), configured to include the following requirements:

1. Exchange eligibility
   - Adherence to the Policy Methodology exchange requirements
   - A minimum rating of BB in CCData's Exchange Benchmark Report. For more information on the Exchange Benchmark, visit here
   - Not classified as an Excluded Exchange as defined in the Policy Methodology

2. Applicable pairs: USD and USDC pairs are included. USDC pairs are converted to USD using the applicable USDC/USD Reference Rate.

CCData Blended Prices VWAP ("Settlement Reference Rates"), are used on the Weighting Reference Date to calculate constituents weights [see Constituent Weighting], to calculate Index Settlement Values [see Index Settlement Calculation Formula] and on the Effective Date to calculate the Index Divisor [see Index Divisor Adjustment]. The Settlement Reference Rates for the Index use the same configuration as the Reference Rates described above.

The methodology for the CCData Blended Reference Prices, referred to as CCIXBER, can be found here.

## Index Calculation Formula

The Index is calculated using the following formula:

Summary of (backtesting) methodology modifications:

| Effective Date | Prior Treatment | Updated Treatment | Material Change |
|----------------|-----------------|-------------------|-----------------|
| Apr 2025 Reconstitution | No custody requirement | Digital asset must be supported by Coinbase Custody Trust Company | Yes |
| Feb 11, 2025 | Reference Rates based on USD-Denominated paris and calculated using an exponentially weighted 30 second lookback period | Reference Rates based on USD and USDC-Denominated pairs and calculated using CCIX's Blended pricing methodology and use Eligible Exchanges that received a rating of BB or higher within CCData's Exchange Benchmark | Yes |
| Jan 2025 Reconstitution | Index Universe sourced from the CoinDesk Market Index | Index Universe sourced from the top 250 digital assets by market capitalization, excluding stablecoins | Yes |
| Dec 18, 2024 | Reconstitution implemented on the 2nd business day of Jan/Apr/Jul/Oct and announced 2 weeks prior | Reconstitution implemented on the last business day of Jan/Apr/Jul/Oct and announced 4 weeks prior | Yes |
| Dec 10, 2024 | Index calculation formula is a weighted return of each constituent's weight as of the prior reconstitution effective date multiplied by its price return since the prior reconstitution effective date. | Index is calculated by summing up the market value of each constituent and dividing by a Divisor | No |
| Aug 27, 2024 | Only CD20 Index available | CD20SPOT Index added as an alternate calculation of CD20 | No |
| July 2024 Reconstitution | Meme coins and privacy tokens eligible for inclusion | Meme coins and privacy tokens are not eligible for inclusion | Yes |


# CoinDesk 5 Index

| Parameter | Value | Def. |
|-----------|--------|------|
| Name | CoinDesk 5 Index | |
| Symbols | Spot: CD5<br>Settlement: CD5SETT | |
| Objective | Measures the market capitalization-weighted performance of the largest five constituents of the CoinDesk 20 Index. | |
| Index Universe | CoinDesk 20 Index (see Methodology) | 1.2 |
| Additional Screening | Not applicable | 2.0 |
| Target Count | Fixed, 5 | 3.1.1 |
| Constituent Selection | Market capitalization | 3.2.1 |
| Buffer Rule | Applicable, 4/6 | 3.2.2 |
| Constituent Weighting | Market capitalization | 3.3.2 |
| Spot Reference Rate | CCIXber | 4.1.1 |
| Settlement Reference Rate | CCIXbervwap | 4.1.2 |
| Reconstitution | Standard | 5.1 |
| FCA Administration | No | |
| Additional Information | The CoinDesk 5 Index was previously the CoinDesk Digital Large Cap Select Index (DLCS). The change in the methodology, to adopt the rules and reference rates described above, are effective as of June 30, 2025 at 4pm Eastern Time. For the methodology prior to June 30, 2025, please refer to the DLCS Methodology. | |

Index Universe

# 1. Index Universe

## 1.1. Top N by Market Capitalization
An asset must be among the largest N assets, as indicated, by market capitalization, excluding stablecoins.

## 1.2. Parent Index 
An asset must be a constituent of the Index indicated. Unless otherwise indicated, when the Index and the Parent Index undergo reconstitution, the Index will re-select after the Parent Index has re-selected.

## 1.3. Universe of N Assets by Industry Tag
For this universe, an asset must be included in a specified Industry Group or Industry Groups in CoinDesk Data's metadata dataset. CoinDesk Data classifies assets through assigning industry tags. The process occurs continuously and includes scans of new token listings, artificial intelligence tools, industry standards, and scheduled manual reviews. The current classification can be found at: https://developers.coindesk.com/documentation/data-api/asset_v1_top_list?groups=CLASSIFICA+TION.

# 2. Additional Screening

## 2.1. Exclusion Rules
As indicated.

## 2.2. Number of Listing Exchanges
Assets must be listed as eligible trading pairs (USD and/or stablecoin denominated pair) on a minimum of 3 eligible exchanges that contribute to the applicable Reference Rate.

## 2.3. Number of Listing Days
Asset must have at least one exchange listing for the previous 90 days prior to (and including) the Reconstitution Reference Date.

## 2.4. Liquidity
Assets must have sufficient trading volume on a minimum of 3 eligible exchanges that contribute to the applicable Reference Rate specified in the Index Term Sheet. Sufficient trading volume, evaluated at the trading pair level, means that the constituent has traded in each of the 30 days leading to and including the Reconstitution Reference Date.

## 2.5. Jurisdiction Access
Assets must have at least one exchange listing available to customers in a certain jurisdiction, e.g. U.S.

## 2.6. Custody Requirements
If indicated, assets must have access to custody services at one or more Required Custodians as defined in the CoinDesk Indices Policy Methodology.

# 3. Index Construction

## 3.1. Target Count
The Index's target number of constituents, as indicated.

### 3.1.1. Fixed Count
The Index has a fixed target number of constituents, which will be relaxed if there is an insufficient number of candidate constituents in the Index Universe.

### 3.1.2. Variable Count
The Index has a variable number of constituents, typically defined within a range with lower and upper bounds. Unless otherwise indicated, if the number of assets meeting the selection criteria is less than the lower bound, the selection criteria will be relaxed; and if the number of assets meeting the selection criteria exceeds the upper bound, the largest assets by market capitalization will be selected till the upper limit is reached.

## 3.2. Constituent Selection
Index constituents are selected using the applicable method below.

### 3.2.1. Market Capitalization
Rank the Index Universe by market capitalization in descending order. Constituents are selected to achieve the target count indicated, applying a selection buffer, as defined below, if applicable. Market capitalization is measured on the Index's Reconstitution Reference Date.

### 3.2.2. Selection Buffer
To mitigate excessive index turnover, a selection buffer rule is applied, if applicable, to relax the inclusion criteria for existing index constituents. The parameters of the buffer rule will be specified as "u/l" (upper/lower) which will be used to apply the rule using the following steps:

1. The top u assets are selected for inclusion.
2. Current constituents ranked from u+1 to l are then selected until the target count is achieved.
3. If the target count is not yet met, the highest-ranked non-constituents are selected until the target count is met.

## 3.3. Constituent Weighting
The weights of constituents are determined using the applicable Settlement Reference Rates on the Weighting Reference Date using one of the methods below, as indicated.

### 3.3.1. Equal Weight
Constituents are equally weighted.

### 3.3.2. Market Capitalization
Constituents are weighted in proportion to their market capitalization.

### 3.3.3. Market Capitalization Weighted with Capping
Constituents are weighted by market capitalization subject to capping requirements, as indicated. Capped weights are determined using the following steps:

1. Determine the uncapped market capitalization weight of each constituent.
2. If the weight of the largest uncapped constituent exceeds the cap value indicated, reduce its weight to the cap value and redistribute the excess weight to all uncapped constituents in proportion to their market capitalization.
3. Repeat the previous step until all remaining constituents' weights do not exceed n%. If multiple (tiered) caps are indicated, apply as indicated, beginning with the largest uncapped constituent.

Weighting Adjustment Factors (WAFs) are determined on the Weighting Reference Date [see Index Maintenance below] to achieve the weights calculated in the process described above, locking in the circulating supply values used to calculate market capitalization. As such, constituent weights as of the Effective Date will drift with constituent price movements between the Weighting Reference Date and the Effective Date.

# 4. Index Calculation

## 4.1. Constituent Pricing
Used as constituent reference rates for index calculation.

### 4.1.1. Spot Reference Rate
As indicated, the reference rate used to calculate the "spot" (streaming) value of the Index.

### 4.1.2. Settlement Reference Rate
As indicated, the reference rate used to calculate the settlement value of the Index. Settlement values are used in reconstitutions [see Index Maintenance] and may be used for other benchmarking purposes.

## 4.2. Index Calculation Formula
Each Index is calculated using the following formula:

[formulas]

`Pi,SETT` is the price of constituent i as of the Effective Date, or other date and time at which the Divisor Adjustment takes place, determined by its Settlement Reference Rate.

The subscript `PC` represents the respective values of the prior index constituents.

The subscript `NC` represents the respective values of the new index constituents following the application of all reconstitution changes.

# 5. Index Maintenance

## 5.1. Reconstitutions
Reconstitutions include reselection of index constituents and application of weighting rules as indicated. Unless otherwise indicated, reconstitutions are conducted quarterly and scheduled so that the Effective Date, defined below, falls on the last business day of January, April, July and October. 

Reconstitutions comprise events on four dates, defined as follows:

1. **Reconstitution Reference Date**: Snapshot date for data used to select Index constituents. This falls two business days before the Announcement Date.

2. **Announcement Date**: The date on which changes to Index constituents are announced. This falls on calendar days as defined in the Index Annex, before the Effective Date, or the closest following business day.

3. **Weighting Reference Date**: The date on which the Index weights and Weighting Adjustment Factors are calculated, as defined above in Section 4.4. This falls m calendar days as defined in the Index Annex, before the Effective Date, or the closest following business day.

4. **Effective Date**: 4 p.m. Eastern Time on the date on which the reconstitution becomes effective.

### Reconstitution Calendar Example

| Event | Definition | Example |
|-------|------------|---------|
| Reconstitution Reference Date | 2 business days prior to the Announcement Date | Dec 31, 2024 |
| Announcement Date | 4 weeks prior to the Effective Date, or if not a business day, the following business day | Jan 3, 2025 |
| Weighting Reference Date | 7 calendar days prior to the Effective Date, or if not a business day, the following business day | Jan 24, 2025 |
| Effective Date | 4 p.m. Eastern Time on the final business day of January, April, July, and October | Jan 31, 2025 |

## 5.2. Index Changes Between Reconstitutions
In addition to the quarterly process, constituents are monitored for potential anomalies and trading disruptions. Out-of-review monitoring, which would require an index modification, only applies in extraordinary circumstances. Incident types that would require one or more index modifications are outlined in the Policy Methodology.

### Additions
Unless otherwise indicated, there will be no additions to the index between reconstitutions.

### Deletions
Unless otherwise indicated, if a constituent is removed from the Index outside of the scheduled reconstitution process it will not be replaced and, therefore, the constituent count may drop below the target number. The weight of the constituent being removed will be redistributed proportionally to all remaining constituents as of the effective date of removal. No recapping will be performed. The impacted constituent will be considered for inclusion at future reconstitutions if it meets the criteria described above.

# 6. Dissemination

## 6.1. Index Values
Index values are calculated 24x7 and are available publicly at coindesk.com and are also available to subscribers via REST and WebSocket APIs.

