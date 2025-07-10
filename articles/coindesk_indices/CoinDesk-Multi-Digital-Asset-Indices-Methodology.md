# CoinDesk Multi Digital Asset Indices Methodology
*June 2025*

## Table of Contents
- [Introduction](#introduction)
- [Multi Digital Asset Indices and Term Sheets](#multi-digital-asset-indices-and-term-sheets)
- [CoinDesk 5 Index](#coindesk-5-index)
- [Definitions](#definitions)
  - [1. Index Universe](#1-index-universe)
  - [2. Additional Screening](#2-additional-screening)
  - [3. Index Construction](#3-index-construction)
  - [4. Index Calculation](#4-index-calculation)
  - [5. Index Maintenance](#5-index-maintenance)
- [Appendix 1: Data Sources](#appendix-1-data-sources)
- [Appendix 2: Methodology Changes](#appendix-2-methodology-changes)

## Introduction

This document provides methodologies for multi digital asset indices (each, an “Index”). Each Index is defined in terms of parameters, noted in each Index’s Term Sheet and defined in the Definitions section herein.

This document was created and is owned by CoinDesk Indices (“CDI”) to achieve each Index’s indicated Objective. Unless otherwise stated, each Index described herein is administered, calculated and maintained by CDI's affiliate, CC Data Limited ("CCData"), an FCA regulated benchmark administrator. References to CDI in this methodology shall be deemed to include CCData.

There may be circumstances or market events which require CDI, in its sole discretion, to deviate from rules stated herein to ensure each Index continues to meet its Objective. This document should be read in conjunction with the CoinDesk Digital Asset Indices Policy Methodology.

## Multi Digital Asset Indices and Term Sheets

| Index Name                            | Symbol | Launch Date  | Base Value | Base Date   | First Value Date |
| :------------------------------------ | :----- | :----------- | :--------- | :---------- | :--------------- |
| [CoinDesk 5 Index](#coindesk-5-index) | CD5    | Jun 26, 2022 | 1000.00    | Apr 4, 2022 | Apr 4, 2022      |
|                                       |        |              |            |             |                  |
|                                       |        |              |            |             |                  |
|                                       |        |              |            |             |                  |

---

## CoinDesk 5 Index

| Parameter | Value | Def. |
| :--- | :--- | :--- |
| **Name** | CoinDesk 5 Index | |
| **Symbols** | Spot: CD5 <br> Settlement: CD5SETT | |
| **Objective** | Measures the market capitalization-weighted performance of the largest five constituents of the CoinDesk 20 Index. | |
| **Index Universe** | CoinDesk 20 Index (see [Methodology](./CoinDesk-20-Index-Methodology.md)) | [1.2](#1.2-parent-index) |
| **Additional Screening** | Not applicable | [2.0](#2-additional-screening) |
| **Target Count** | Fixed, 5 | [3.1.1](#3.1.1-fixed-count) |
| **Constituent Selection** | Market capitalization | [3.2.1](#3.2.1-market-capitalization) |
| **Buffer Rule** | Applicable, 4/6 | [3.2.2](#3.2.2-selection-buffer) |
| **Constituent Weighting** | Market capitalization | [3.3.2](#3.3.2-market-capitalization) |
| **Spot Reference Rate** | CCIXber | [4.1.1](#4.1.1-spot-reference-rate) |
| **Settlement Reference Rate** | CCIXbervwap | [4.1.2](#4.1.2-settlement-reference-rate) |
| **Reconstitution** | Standard | [5.1](#5.1-reconstitutions) |
| **FCA Administration** | No | |
| **Additional Information** | The CoinDesk 5 Index was previously the CoinDesk Digital Large Cap Select Index (DLCS). The change in the methodology, to adopt the rules and reference rates described above, are effective as of June 30, 2025 at 4pm Eastern Time. For the methodology prior to June 30, 2025, please refer to the DLCS Methodology. | |

---

## Definitions

### 1. Index Universe

#### 1.1. Top N by market capitalization

An asset must be among the largest N assets, as indicated, by market capitalization¹, excluding stablecoins.

#### 1.2. Parent Index

An asset must be a constituent of the Index indicated. Unless otherwise indicated, when the Index and the Parent Index undergo reconstitution, the Index will re-select after the Parent Index has re-selected.

#### 1.3. Universe of N assets by industry tag

For this universe, an asset must be included in a specified Industry Group or Industry Groups in CoinDesk Data’s metadata dataset. CoinDesk Data classifies assets through assigning industry tags. The process occurs continuously and includes scans of new token listings, artificial intelligence tools, industry standards, and scheduled manual reviews. The current classification can be found at the following endpoint: <https://developers.coindesk.com/documentation/data-api/asset_v1_top_list?groups=CLASSIFICA+TION>.

### 2. Additional Screening

#### 2.1. Exclusion rules

As indicated.

#### 2.2. Number of listing exchanges

Assets must be listed as eligible trading pairs (USD and/or stablecoin denominated pair) on a minimum of 3 eligible exchanges that contribute to the applicable Reference Rate.

#### 2.3. Number of listing days

Asset must have at least one exchange listing for the previous 90 days prior to (and including) the Reconstitution Reference Date.

#### 2.4. Liquidity

Assets must have sufficient trading volume on a minimum of 3 eligible exchanges that contribute to the applicable Reference Rate specified in the Index Term Sheet. Sufficient trading volume, evaluated at the trading pair level, means that the constituent has traded in each of the 30 days leading to and including the Reconstitution Reference Date

¹ Throughout this document, market capitalization is calculated as circulating supply multiplied by price.

#### 2.5. Jurisdiction access

Assets must have at least one exchange listing available to customers in a certain jurisdiction, e.g. U.S.

#### 2.6. Custody requirements

If indicated, assets must have access to custody services at one or more Required Custodians as defined in the CoinDesk Indices Policy Methodology.

### 3. Index Construction

#### 3.1. Target count

The Index’s target number of constituents, as indicated.

##### 3.1.1. Fixed count

The Index has a fixed target number of constituents, which will be relaxed if there is an insufficient number of candidate constituents in the Index Universe.

##### 3.1.2. Variable count

The Index has a variable number of constituents, typically defined within a range with lower and upper bounds. Unless otherwise indicated, if the number of assets meeting the selection criteria is less than the lower bound, the selection criteria will be relaxed; and if the number of assets meeting the selection criteria exceeds the upper bound, the largest assets by market capitalization will be selected till the upper limit is reached.

#### 3.2. Constituent selection

Index constituents are selected using the applicable method below.

##### 3.2.1. Market capitalization

Rank the Index Universe by market capitalization in descending order. Constituents are selected to achieve the target count indicated, applying a selection buffer, as defined below, if applicable. Market capitalization is measured on the Index’s Reconstitution Reference Date.

##### 3.2.2. Selection buffer

To mitigate excessive index turnover, a selection buffer rule is applied, if applicable, to relax the inclusion criteria for existing index constituents. The parameters of the buffer rule will be specified as “u/l” (upper/lower) which will be used to apply the rule using the following steps:

**Step 1:** The top u assets are selected for inclusion.
**Step 2:** Current constituents ranked from u+1 to l are then selected until the target count is achieved.
**Step 3:** If the target count is not yet met, the highest-ranked non-constituents are selected until the target count is met.

#### 3.3. Constituent Weighting

The weights of constituents are determined using the applicable Settlement Reference Rates on the Weighting Reference Date using one of the methods below, as indicated.

##### 3.3.1. Equal weight

Constituents are equally weighted.

##### 3.3.2. Market capitalization

Constituents are weighted in proportion to their market capitalization.

##### 3.3.3. Market capitalization weighted with capping

Constituents are weighted by market capitalization subject to capping requirements, as indicated. Capped weights are determined using the following steps:

**Step 1:** Determine the uncapped market capitalization weight of each constituent.
**Step 2:** If the weight of the largest uncapped constituent exceeds the cap value indicated, reduce its weight to the cap value and redistribute the excess weight to all uncapped constituents in proportion to their market capitalization.
**Step 3:** Repeat the previous step until all remaining constituents’ weights do not exceed n%. If multiple (tiered) caps are indicated, apply as indicated, beginning with the largest uncapped constituent.

Weighting Adjustment Factors (WAFs) are determined on the Weighting Reference Date [see Index Maintenance below] to achieve the weights calculated in the process described above, locking in the circulating supply values used to calculate market capitalization. As such, constituent weights as of the Effective Date will drift with constituent price movements between the Weighting Reference Date and the Effective Date

### 4. Index Calculation

#### 4.1. Constituent pricing

Used as constituent reference rates for index calculation.

##### 4.1.1. Spot Reference Rate

As indicated, the reference rate used to calculate the “spot” (streaming) value of the Index.

##### 4.1.2. Settlement Reference Rate

As indicated, the reference rate used to calculate the settlement value of the Index. Settlement values are used in reconstitutions [see Index Maintenance] and may be used for other benchmarking purposes.

#### 4.2. Index Calculation Formula

Each Index is calculated using the following formula:

\[
Index_t = \frac{\sum_{i=1}^{N} P_{i,t} \times S_i \times WAF_i}{Divisor}
\]

where,

- **Index_t** is the value of the Index at time t,
- **P_i,t** is the price of constituent i at time t, as determined by its Spot Reference Rate,
- **S_i** refers to the circulating supply of constituent i as of the Weighting Reference Date,
- **WAF_i** is the Weighting Adjustment Factor of constituent i, as of the Weighting Reference Date,
- **Divisor** is the Index Divisor.

#### 4.3. Index Settlement Calculation Formula

The Index Settlement value is calculated using the following formula:

\[
Index_{SETT} = \frac{\sum_{i=1}^{N} P_{i,SETT} \times S_i \times WAF_i}{Divisor}
\]

where,

- **Index_SETT** is the settlement value of the Index,
- **P_i,SETT** is the price of constituent i as determined by its Settlement Reference Rate.

#### 4.4. Index Divisor Adjustment

The Index Divisor is recalculated using the following formula on each reconstitution Effective Date and during any event which requires a change to the index constituents not driven solely by market price movements.

\[
Divisor_{NC} = Divisor_{PC} \times \frac{\sum_{i=1}^{N} P_{i,SETT} \times S_{i, NC} \times WAF_{i, NC}}{\sum_{i=1}^{N} P_{i,SETT} \times S_{i, PC} \times WAF_{i, PC}}
\]

**P_i,SETT** is the price of constituent i as of the Effective Date, or other date and time at which the Divisor Adjustment takes place, determined by its Settlement Reference Rate,
The subscript **PC** represents the respective values of the prior index constituents,
The subscript **NC** represents the respective values of the new index constituents following the application of all reconstitution changes.

### 5. Index Maintenance

#### 5.1. Reconstitutions

Reconstitutions include reselection of index constituents and application of weighting rules as indicated. Unless otherwise indicated, reconstitutions are conducted quarterly and scheduled so that the Effective Date, defined below, falls on the last business day of January, April, July and October. Reconstitutions comprise events on four dates, defined as follows:

1.  **Reconstitution Reference Date:** Snapshot date for data used to select Index constituents. This falls two business days before the Announcement Date.
2.  **Announcement Date:** The date on which changes to Index constituents are announced. This falls on calendar days as defined in the Index Annex, before the Effective Date, or the closest following business day.
3.  **Weighting Reference Date:** The date on which the Index weights and Weighting Adjustment Factors are calculated, as defined above in Section 4.4. This falls m calendar days as defined in the Index Annex, before the Effective Date, or the closest following business day.
4.  **Effective Date:** 4 p.m. Eastern Time on the date on which the reconstitution becomes effective.

##### Reconstitution Calendar Example
| Event | Definition | Example |
| :--- | :--- | :--- |
| **Reconstitution Reference Date** | 2 business days prior to the Announcement Date | Dec 31, 2024 |
| **Announcement Date** | 4 weeks prior to the Effective Date, or if not a business day, the following business day | Jan 3, 2025 |
| **Weighting Reference Date** | 7 calendar days prior to the Effective Date, or, if not a business day, the following business day | Jan 24, 2025 |
| **Effective Date** | 4 p.m. Eastern Time on the final business day of January, April, July, and October | Jan 31, 2025 |

#### 5.2. Index changes between reconstitutions

In addition to the quarterly process, constituents are monitored for potential anomalies and trading disruptions. Out-of-review monitoring, which would require an index modification, only applies in extraordinary circumstances. Incident types that would require one or more index modifications are outlined in the Policy Methodology.

**Additions:** Unless otherwise indicated, there will be no additions to the index between reconstitutions.

**Deletions:** Unless otherwise indicated, if a constituent is removed from the Index outside of the scheduled reconstitution process it will not be replaced and, therefore, the constituent count may drop below the target number. The weight of the constituent being removed will be redistributed proportionally to all remaining constituents as of the effective date of removal. No recapping will be performed. The impacted constituent will be considered for inclusion at future reconstitutions if it meets the criteria described above.

### 6. Dissemination

#### 6.1.

Index values are calculated 24x7 and are available publicly at coindesk.com and are also available to subscribers via REST and WebSocket APIs.

---

## Appendix 1: Data Sources

This section describes data sources used to maintain, rebalance, and calculate the products described herein since the base date. If data are not available for any reason from the sources described in this appendix, other data sources may be used.

---

## Appendix 2: Methodology Changes

The table below is a summary of modifications to this Methodology.

| Index | Effective Date | Prior Treatment | Updated Treatment | Material Change |
| :--- | :--- | :--- | :--- | :--- |
| CoinDesk 5 | Jun 30, 2025 | DLCS methodology² | CoinDesk 5 methodology | Yes |
| CoinDesk 5 | Jun 5, 2025 | Name: CoinDesk Large Cap Index<br>Ticker: DLCS | Name: CoinDesk 5 Index<br>Ticker: CD5 | No |

² For the methodology prior to June 30, 2025, please refer to the DLCS Methodology.
