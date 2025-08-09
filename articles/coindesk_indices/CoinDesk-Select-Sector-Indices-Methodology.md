# CoinDesk Select Sector Indices Methodology
_February 2025_

## Table of Contents
- Introduction
    - Index Objective
    - Additional Details
    - Table 1: Index Details
- Eligibility Criteria
    - Index Universe Eligibility
    - Table 2: DACS Sector Eligibility
- Index Construction
    - Constituent Selection
    - Table 3: Selection Requirements
    - Constituent Weighting
- Index Calculation
    - Constituent Pricing
    - Index Calculation Formula
    - Index Divisor Adjustment
- Index Maintenance
    - Index Reconstitution
    - Table 4: Reconstitution Calendar Example
    - Index Changes Between Reconstitutions
    - Additions
    - Deletions
    - DACS Reclassification
    - Data Distribution
    - Index Governance
- Appendix 1: Methodology Changes
- Appendix 2: Data Sources
    - Reconstitution data
    - Pricing Data
- Appendix 3: CoinDesk Smart Contract Platform Select Capped Index
- Appendix 4: CoinDesk Smart Contract Platform Select Ex ETH Index
- Appendix 5: CoinDesk Currency Select Ex Bitcoin Index
- Appendix 6: CoinDesk Market Select Index
- Appendix 7: Document Revision History
- Disclaimer

## Introduction
### Index Objective
The CoinDesk Select Sector Index Series (“The Indices”) measures the market-capitalization weighted performance of some of the largest and most liquid digital assets classified in eligible DACS sectors that meet certain trading and custody requirements [see Index Universe Eligibility].

### Additional Details
The CoinDesk Select Sector Index series (the “Indices”) are based on the Digital Asset Classification Standard (DACS). Constituents of each index must be included in DACS and assigned to the appropriate Sector. For more information on DACS please refer to the DACS Methodology.

This methodology was created by CoinDesk Indices (“CDI”) to achieve the Index Objective stated above. There may be circumstances or market events which require CDI, in its sole discretion, to deviate from these rules to ensure each Index continues to meet its Index Objective. This document should be read in conjunction with the CoinDesk Digital Asset Indices Policy Methodology.

Appendix 1 provides a summary of methodology changes impacting the Indices.

Appendix 2 provides the data sources used to manage and reconstitute the indices.

Table 1 provides the list of indices as determined by CoinDesk Indices. This list may expand in the future at the discretion of CDI.

### Table 1: Index Details
| Index Name | Index Ticker | Launch Date | Base Date | Base Value |
| :--- | :--- | :--- | :--- | :--- |
| CoinDesk Smart Contract Platform Select Index | SCPX | Mar 15, 2022 | Mar 10, 2022 | 1000.00 |
| CoinDesk Smart Contract Platform Select Ex ETH Index | SCPXX | Mar 15, 2022 | Mar 10, 2022 | 1000.00 |
| CoinDesk Smart Contract Platform Select Capped Index | SCPXC | Feb 12, 2025 | Mar 10, 2022 | 1000.00 |
| CoinDesk DeFi Select Index | DFX | Jul 19, 2021 | May 10, 2021 | 1000.00 |
| CoinDesk Currency Select Index | CCYS | Aug 30, 2022 | Jul 18, 2022 | 1000.00 |
| CoinDesk Currency Select Ex Bitcoin Index | CCYX | Aug 30, 2022 | Jul 18, 2022 | 1000.00 |
| CoinDesk Computing Select Index | CPUS | Aug 30, 2022 | Jul 18, 2022 | 1000.00 |
| CoinDesk Culture & Entertainment Select Index | CNES | Aug 30, 2022 | Jul 18, 2022 | 1000.00 |
| CoinDesk Market Select Index | CMIS | Dec 22, 2022 | Jul 18, 2022 | 1000.00 |

SCPXC, SCPXX, CCYX and CMIS are variations of the Select Sector Indices. See Appendices 3, 4, 5, and 6 for additional details on these indices.

## Eligibility Criteria
### Index Universe Eligibility
To be included in an Index Universe, a digital asset must meet the following criteria as of the Reconstitution Reference Date:
1. The digital asset must be included in the most recently published DACS report and assigned to the appropriate DACS Sector as defined in Table 2.
2. Custodian services for the digital asset must be available from Coinbase Custody, a division of Coinbase Global Inc. and the digital asset must be accessible by U.S. investors.
3. The digital asset must not be categorized as a memecoin as determined by CDI.
4. The digital asset must have traded on at least one Eligible Exchange within the last 30 days up to and including the Reconstitution Reference Date. For details on Eligible Exchanges, refer to the CoinDesk Digital Asset Indices Policy Methodology.

The Index Committee reserves the right to relax the eligibility criteria if an insufficient number of digital assets qualify.

### Table 2: DACS Sector Eligibility
| Index Name | DACS Sector Name |
| :--- | :--- |
| CoinDesk Smart Contract Platform Select Index | Smart Contract Platform |
| CoinDesk DeFi Select Index | Decentralized Finance (“DeFi”) |
| CoinDesk Computing Select Index | Computing |
| CoinDesk Currency Select Index | Currency |
| CoinDesk Culture & Entertainment Select Index | Culture & Entertainment |

## Index Construction
### Constituent Selection
The constituent selection process targets the largest and most liquid digital assets from the Selection Universe subject to a final constituent count between 5 and 10 inclusive, and buffer rules designed to reduce unnecessary turnover.

All digital assets that pass the Eligibility Criteria will be ranked by market capitalization, as of the Reconstitution Reference Date, in descending order. The 20 highest ranked digital assets that meet the Eligibility Criteria form the Selection Universe.

Constituents will be selected from the Selection Universe as follows:
1. For each digital asset in the Selection Universe, calculate the Median Daily Value Traded (MDVT) across eligible exchanges over the previous 30-day calendar period leading up to and including the Reconstitution Reference Date.
2. For each digital asset in the Selection Universe, determine the closing market capitalization as of the Reconstitution Reference Date.
3. Based on the results from Step 1 and Step 2, the market cap and liquidity requirements are determined as follows:
    a. Non-Constituent Liquidity Requirement = 1.20 times the median MDVT of the Selection Universe
    b. Non-Constituent Market Cap Requirement = 1.20 times the median closing market cap of the Selection Universe.
    c. Constituent Liquidity Requirement = median MDVT of the Selection Universe
    d. Constituent Market Cap Requirement = median closing market cap of the Selection Universe
4. Following the determination of the market cap and liquidity requirements, all digital assets that do not meet all of the following criteria as of the reconstitution reference date will be considered ineligible and, therefore, removed from the Selection Universe:
    a. Must be a constituent of the CoinDesk Market Index (“CMI”). For more information on the CMI, please refer to the CoinDesk Market Index Methodology.
    b. Must be supported by a CoinDesk Reference Rate with 3 contributing exchanges.
    c. Must be included on the Watchlist (as determined during the last reconstitution).
5. All digital assets in the Selection Universe following Step 4 that were included in the Watchlist will be evaluated against the Non-Constituent Liquidity and Non-Constituent Market Cap Requirements. All current constituents in the Selection Universe will be evaluated against the Constituent Liquidity and Constituent Market Cap Requirements. Digital assets that meet or exceed these requirements will be selected as index constituents (see Table 3). Based on the following results:
    a. If fewer than 6 digital assets are selected, the market cap and liquidity requirements for non-constituents will be relaxed to the requirements for constituents (Note: the digital asset must be included on the Watchlist and in the Selection Universe to be selected).
    b. If more than 10 digital assets are selected, the digital asset(s) with the smallest market cap will be removed until 10 digital assets remain.
6. If Step 5 results in at least 5 digital assets selected as constituents, the selection process is complete. Otherwise, the market cap and liquidity requirements will be relaxed for constituents and non-constituents until a minimum of 5 constituents are selected. If there are fewer than 5 constituents in the Selection Universe, all digital assets in the Selection Universe will be selected and the constituent count will drop below the minimum of 5.

The Index Committee will review the constituent selection results if the market cap and liquidity criteria need to be relaxed.

### Table 3: Selection Requirements
| Requirement for Inclusion | Non-constituents | Constituents |
| :--- | :--- | :--- |
| Included in Selection Universe | Yes | Yes |
| Included on latest Watchlist | Yes | No |
| Minimum Markets Test | 3 eligible exchanges | 3 eligible exchanges |
| Closing Market Cap | >= median Market Cap of the Selection Universe * 1.20 | >= median Market Cap of the Selection Universe |
| 30-day Median Daily Value Traded (MDVT) | >= median 30-day MDVT of the Selection Universe * 1.20 | >= median 30-day MDVT of the Selection Universe |

### Constituent Weighting
Constituents are market capitalization weighted subject to a minimum weight requirement of one percent. The following process is used to weight constituents at each reconstitution:
1. Determine the preliminary weights of each digital asset selected based on its market capitalization as a percentage of the total market capitalization of all digital assets selected.
2. Rank all digital assets from Step 1 in descending order based on the preliminary weight.
3. To minimize unnecessary turnover, the Minimum Weight Requirement for all current constituents (non-constituents) is 0.80 (1.0) percent respectively. The preliminary weights from Step 2 will be reviewed against the Minimum Weight Requirement.
4. If the preliminary weight of any digital asset from Step 2 is below the applicable Minimum Weight Requirement determined in Step 3 and there are more than 5 digital asset constituents, the following iterative steps are performed:
    a. Remove the lowest ranked digital asset and redistribute its weights proportionally to all remaining digital assets.
    b. Repeat Step 4 if the weight of any remaining digital asset is below the applicable Minimum Weight Requirement and there are more than 5 digital asset constituents.
5. The results determined in Step 4 will be the final constituents and weights.

## Index Calculation
The Indices are calculated in real time using the applicable Reference Rates for underlying constituents.

### Constituent Pricing
Reference Rates for constituent digital assets are calculated approximately every 5 seconds using a volume weighted average price (VWAP) across three contributing exchanges over the prior 60 minutes. These reference rates are referred to as Settlement Reference Rates in the CoinDesk Reference Rate Methodology.

The number of contributing exchanges may drop below three between reconstitutions. In the event the number of contributing exchanges drops below three between reconstitutions, the Index Committee will review the eligible exchanges to determine the appropriate course of action. In general, changes to contributing exchanges will only be implemented during the quarterly reconstitution.

### Index Calculation Formula
The Index is calculated using the following formula:
\\[
Index_t = \\frac{\\sum_{i=1}^{N} P_{i,t} \\times S_i \\times WAF_i}{Divisor}
\\]
where,

-   `Index_t` is the value of the Index at time t
-   `P_{i,t}` is the price of constituent i at time t, as determined by its Reference Rate,
-   `S_i` is the circulating supply of constituent i as of the Weighting Reference Date,
-   `WAF_i` is the Weighting Adjustment Factor[^1] of constituent i, as of the Weighting Reference Date,
-   `Divisor` is the Index Divisor.

[^1]: For indices that are weighed by market capitalization with no other adjustments, the WAF will be set to 1.0000.

### Index Divisor Adjustment
The Index Divisor is recalculated on each reconstitution Effective Date and during any event which requires a change to the index constituents not driven solely by market price movements.
\\[
Divisor_{NC} = Divisor_{PC} \\times \\frac{\\sum_{i=1}^{N} P_{i,SETT} \\times S_{i,NC} \\times WAF_{i,NC}}{\\sum_{i=1}^{N} P_{i,SETT} \\times S_{i,PC} \\times WAF_{i,PC}}
\\]
where,

-   `P_{i,SETT}` is the price of constituent i as of the Effective Date, or other date and time at which the Divisor Adjustment takes place, determined by its Settlement Reference Rate,
-   The subscript `PC` represents the respective values of the prior index constituents,
-   The subscript `NC` represents the respective values of the new index constituents following the application of all reconstitution changes.

## Index Maintenance

### Index Reconstitution
The Index is reviewed and reconstituted on a quarterly basis based on the rules described above [see Index Construction]. Any index changes resulting from the quarterly review are announced two weeks prior to the Effective Date, at which time the changes are implemented. Settlement Reference Rates, as defined above [see Constituent Pricing] are used for implementation on the Effective date. Reconstitutions are scheduled so that the Effective Dates fall on the second business day[^2] of January, April, July and October. Reconstitutions include four events, defined as follows:

1.  **Reconstitution Reference Date.** Snapshot date for data used to select Index constituents. This falls two business days before the Announcement Date.
2.  **Announcement Date.** The date on which changes to Index constituents are announced. This falls fourteen calendar days before the Effective Date, or the closest following business day.
3.  **Weighting Reference Date.** The date on which the Index weights and Weighting Adjustment Factors are calculated, as defined above [see Index Construction]. This falls seven calendar days before the Effective Date, or the closest following business day.
4.  **Effective Date.** 4 p.m. Eastern Time on the date on which the reconstitution becomes effective.

[^2]: Business days are defined in the Policy Methodology.

### Table 4: Reconstitution Calendar Example
| Activity | Timing | Example |
| :--- | :--- | :--- |
| Reconstitution Reference Date | 2 business days prior to the Announcement Date | Dec 18, 2024 |
| Announcement Date | 2 weeks prior to the Effective Date, or if not a business day, the following business day | Dec 20, 2024 |
| Weighting Reference Date | 7 calendar days prior to the Effective Date, or, if not a business day, the following business day | Dec 27, 2024 |
| Effective Date | 4 p.m. Eastern Time on the second business day of January, April, July, and October | Jan 3, 2025 |

### Index Changes Between Reconstitutions
In addition to the quarterly process, constituents are monitored for potential anomalies and trading disruptions. Out-of-review monitoring, which would require an index modification, only applies in extraordinary circumstances. Incident types that would require one or more index modifications are outlined in the Policy Methodology.

#### Additions
There will be no additions to the Indices between reconstitutions.

#### Deletions
If a constituent is removed from one or more of the Indices outside of the scheduled reconstitution process it will not be replaced and, therefore, the constituent count may drop below the target number. The weight of the constituent being removed will be redistributed proportionally to all remaining constituents as of the effective date of removal. No recapping will be performed for indices subject to capping. The impacted constituent will be considered for inclusion at future reconstitutions if it meets the criteria described above.

### DACS Reclassification
In the event the DACS sector is modified for an existing index constituent between reconstitutions, the impacted constituent will remain in the index until the next scheduled reconstitution. Based on the new sector assignment, the impacted digital asset will be added automatically to the Watchlist for the relevant Select Sector index, if one exists, and be reviewed at the next reconstitution for possible inclusion.

### Data Distribution
Index values are calculated 24x7 and are available publicly at coindesk.com and are also available to subscribers via REST and WebSocket APIs.

### Index Governance
Pursuant to CDI’s arrangement with its affiliate CCData to perform administration and calculation services, the Indices are subject to CCData’s governance and oversight functions. For more details on CCData, see here. These provisions override the governance and oversight provisions in the Policy methodology.

## Appendix 1: Methodology Changes
The table below is a summary of modifications to this Methodology.

| Effective Date | Prior Treatment | Updated Treatment | Material Change |
| :--- | :--- | :--- | :--- |
| Dec 10, 2024 | Index calculation formula is a weighted return of each constituent’s weight as of the prior reconstitution effective date multiplied by is price return since the prior reconstitution effective date. | Index is calculated by summing up the market value of each constituent and dividing by a Divisor | No |
| May 20, 2024 | Digital assets must be ranked within the top 200 of DACS to be eligible. | Digital assets must be included in DACS to be eligible. | No |
| Dec 21, 2021 | N/A | Digital asset must be included in CMI to be eligible for inclusion. | No |
| Dec 21, 2021 | Weighting Reference Date 2 business prior to Effective Date | Weighting Reference Date 7 calendar day prior to Effective Date | No |
| July 2022 Recon | CoinDesk DeFi Index (DFX) followed its own Index Methodology (available upon request). | Index renamed to CoinDesk DeFi Select Index (DFX) and follows the CoinDesk Select Sector Methodology. | Yes |
| July 2022 Recon | Minimum weight requirement of 1.0% for constituents and non-constituents. | Minimum weight requirement buffer introduced as follows: Constituents = 0.8% Non-constituents = 1.0% | No |
| Jun 26, 2022 | Constituents are re-weighted daily at 4PM using a 24-hour VWAP. This weight is used for the following 24-hour calculation. | Constituent weights are reset at each reconstitution and the calculation formula is modified to a weighted return algorithm based on each constituent’s performance since the prior reconstitution. | Yes |

## Appendix 2: Data Sources
This section describes data sources used to maintain, reconstitute, and calculate the Indices since the initial base date. If data are not available for any reason from the sources described in this appendix, other data sources may be used.

### Reconstitution data
*   Market capitalization, snapshot pricing and circulating supply data are sourced from CCData at midnight UTC on the Reference Date.
*   Volume data for liquidity analysis is sourced from CCData each day of the defined lookback period.

Volume, pricing, classifications and reference data used for reconstitutions are reviewed by CoinDesk Indices analysts for accuracy and reliability. Based on these reviews, CoinDesk Indices reserves the right to update reconstitution data.

### Pricing Data
Pricing for constituents used to calculate the index is sourced from underlying exchanges and trading venues.

## Appendix 3: CoinDesk Smart Contract Platform Select Capped Index
The CoinDesk Smart Contract Platform Select Capped Index (“SCPXC”) measures the market capitalization weighted performance of the constituents of the CoinDesk Smart Contract Platform Select Index (“SCPX”) with a 30% cap on index constituents.

SCPXC follows the same maintenance and reconstitution schedule as SCPX.

### Constituent Selection
At each reconstitution, the new list of constituents for SCPX are selected and form the index.

### Constituent Weighting
Constituents are weighted by market capitalization subject to a 30% cap. The capping is imposed at each reconstitution using the following steps:
1.  Determine the uncapped market capitalization weight of each constituent using pricing determined by the applicable Settlement Reference Rates as of the Weighting Reference Date.
2.  Rank the results from the previous step in descending order.
3.  If the weight of the largest uncapped constituent exceeds 30%, reduce its weight to 30% and redistribute the excess weight to all uncapped constituents in proportion to their market capitalization.
4.  Repeat the previous step until all remaining constituents’ weights do not exceed 30%.

Weighting Adjustment Factors are determined on the Weighting Reference Date[^3] [see Index Maintenance above] to achieve the weights calculated in the process described above, locking in the circulating supply values used to calculate market capitalization. As such, constituent weights as of the Effective Date will drift with constituent price movements between the Weighting Reference Date and the Effective Date.

[^3]: For the historical backtest of SCPXC, Weighting Adjustment Factors were determined on the Effective Date of each reconstitution.

## Appendix 4: CoinDesk Smart Contract Platform Select Ex ETH Index
The CoinDesk Smart Contract Platform Select Ex ETH Index (“SCPXX”) measures the market capitalization weighted performance of the constituents of the CoinDesk Smart Contract Platform Select Index (“SCPX”) excluding Ethereum.

SCPXX follows the same maintenance and reconstitution schedule as SCPX.

At each reconstitution, the new list of constituents for SCPX, excluding Ethereum, are selected and form the index. The weight of Ethereum is proportionally redistributed to the remaining constituents. The final constituents are weighted by market capitalization.

## Appendix 5: CoinDesk Currency Select Ex Bitcoin Index
The CoinDesk Currency Select Ex Bitcoin Index (“CCYX”) measures the market capitalization weighted performance of the constituents of the CoinDesk Currency Select Index (“CCYS”) excluding Bitcoin.

CCYX follows the same maintenance and reconstitution schedule as CCYS.

At each reconstitution, the new list of constituents for CCYS, excluding Bitcoin, are selected and form the index. The weight of Bitcoin is proportionally redistributed to the remaining constituents. The final constituents are weighted by market capitalization.

## Appendix 6: CoinDesk Market Select Index
The CoinDesk Market Select Index (“CMI Select”) measures the market capitalization weighted performance of the constituents of the CoinDesk Select Sector Indices listed below:

| Index Name | Index Ticker |
| :--- | :--- |
| CoinDesk Smart Contract Platform Select Index | SCPX |
| CoinDesk DeFi Select Index | DFX |
| CoinDesk Currency Select Index | CCYS |
| CoinDesk Computing Select Index | CPUS |
| CoinDesk Culture & Entertainment Select Index | CNES |

### Index Maintenance
The constituents of the CMI Select are the constituents of the underlying CoinDesk Select Sector Indices listed above. CMI Select follows the same maintenance, calculation, and reconstitution procedures as the underlying indices.

### Constituent Weighting
Constituents are market capitalization weighted.

### Additions
In the event a new select sector index becomes available, the Index Committee will determine whether the index will be added as a constituent of the CMI Select.

### Deletions
CoinDesk Indices reserves the right to remove a constituent from the CMI Select in the event the underlying index is terminated or is no longer representative.

## Appendix 7: Document Revision History
| Timing | Description |
| :--- | :--- |
| Feb 12, 2025 | Addition of Smart Contract Platform Select Capped Index (SCPXC)<br/>Minor updates and clarifications |
| Dec 10, 2024 | Update to calculation formula to use a divisor methodology. |
| Aug 23, 2024 | Updated Constituent Pricing section to clarify that pricing uses the Settlement Reference Rates in the Reference Rate Methodology following the addition of Spot Reference Rates |
| May 9, 2024 | See Methodology Changes Appendix dated 5/20/2024. In addition,<br/>minor updates and clarifications as part of the Annual Methodology Review |
| Jul 21, 2023 | Minor updates and clarifications |
| Mar 6, 2023 | Annual Methodology Review including minor updates and clarifications |
| Dec 21, 2022 | -Updates to Index Selection language<br/>-Removed reference to stablecoins in Eligibility section as stablecoins is a separate sector in DACS.<br/>-Updates to Constituent Pricing section to refer to the CoinDesk Reference Rate methodology<br/>-Addition of new index – CMIS<br/>-Updates to Data Sources appendix related to determination of notional volume |
| May 4, 2022 | Inclusion of DFX in document + minor edits |
| Aug 29, 2022 | Addition of CCYS, CCYX, CPUS, CNES Indices |
| Jun 14, 2022 | Updates to methodology included in Methodology Change table effective June 25, 2022; removal of Reference Rate Appendix; other clarifications and minor updates to Constituent Selection and Watchlist sections. |
| May 4, 2022 | Inclusion of DFX in document + minor edits |
| Apr 4, 2022 | Revision: Update Reference Rate Contributing Exchanges |
| Mar 12, 2022 | Initial Version |

