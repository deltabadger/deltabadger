# CoinDesk Market Index (CMI) Methodology

*December 2024*

## Table of Contents

- [Introduction](#introduction)
  - [Index Objective](#index-objective)
  - [Additional details](#additional-details)
- [Eligibility Criteria](#eligibility-criteria)
  - [Universe eligibility](#universe-eligibility)
- [Index Construction](#index-construction)
  - [Constituent selection](#constituent-selection)
  - [Constituent weighting](#constituent-weighting)
  - [Circulating supply](#circulating-supply)
  - [Index calculation](#index-calculation)
  - [Constituent pricing](#constituent-pricing)
  - [Calculation Formula](#calculation-formula)
  - [Index Divisor Adjustment](#index-divisor-adjustment)
- [Index Maintenance](#index-maintenance)
  - [Index reconstitution](#index-reconstitution)
  - [Deletions](#deletions)
  - [Additions](#additions)
  - [DACS reclassifications](#dacs-reclassifications)
- [Data Distribution](#data-distribution)
- [Index Governance](#index-governance)
- [Appendix 1: Data sources](#appendix-1-data-sources)
- [Appendix 2: CMI Family Backcast Methodology](#appendix-2-cmi-family-backcast-methodology)
- [Appendix 3: Methodology Changes](#appendix-3-methodology-changes)

## Introduction

### Index Objective

The CoinDesk Market Indices (the “Indices”) are a family of broad-based digital asset indices designed to measure the performance of the digital asset market. The family includes indices that reflect sectors defined in CoinDesk’s Digital Asset Classification Standard (“DACS”). Constituents must meet certain trading day requirements and trade on eligible exchanges as further detailed in Eligibility Criteria below. The Indices are weighted by market capitalization.

### Additional details

The flagship CoinDesk Market Index (“CMI”) is a broad representation of the performance of the digital asset market excluding stablecoins.

The Indices may include digital assets that are stablecoins, relatively illiquid or otherwise difficult to access, own, trade or custody. Digital assets that meet all eligibility criteria will be included regardless of their designation as a security by a U.S. government oversight agency. See Eligibility Criteria below for more details.

The Indices are derived from the Digital Asset Classification Standard (DACS). Constituents must be included in DACS to be eligible for inclusion. For more information on DACS, please refer to the DACS Glossary and DACS Methodology.

This methodology was created by CDI to achieve the above Index Objective. There may be circumstances or market events which require CDI, in its sole discretion, to deviate from certain rules to ensure each Index continues to meet its objective. This document should be read in conjunction with the CoinDesk Digital Asset Indices Policy Methodology.

### Table 1: List of Indices

| Index Name                               | Ticker | Launch Date | Base Date  | Base Value | First Value Date¹ |
| ---------------------------------------- | ------ | ----------- | ---------- | ---------- | ----------------- |
| CoinDesk Market Index                    | CMI    | 9/9/2022    | 8/29/2022  | 1000.00    | 12/29/2017        |
| CoinDesk Market Plus Stablecoin Index    | CMIP   | 9/9/2022    | 8/29/2022  | 1000.00    | 12/29/2017        |
| CoinDesk Computing Index                 | CPU    | 9/9/2022    | 8/29/2022  | 1000.00    | 12/29/2017        |
| CoinDesk Culture & Entertainment Index   | CNE    | 9/9/2022    | 8/29/2022  | 1000.00    | 2/2/2018          |
| CoinDesk Currency Index                  | CCY    | 9/9/2022    | 8/29/2022  | 1000.00    | 12/29/2017        |
| CoinDesk DeFi Index                      | DCF    | 9/9/2022    | 8/29/2022  | 1000.00    | 6/4/2018          |
| CoinDesk Digitization Index              | DTZ    | 9/9/2022    | 8/29/2022  | 1000.00    | 10/4/2021         |
| CoinDesk Smart Contract Platform Index   | SMT    | 9/9/2022    | 8/29/2022  | 1000.00    | 12/29/2017        |
| CoinDesk Stablecoin Index                | CSC    | 12/21/2022  | 8/29/2022  | 1000.00    | 2/4/2019          |

¹data prior to 8/29/2022 is backcasted history. See Backcast Methodology Appendix

## Eligibility Criteria

### Universe eligibility

To be included in the Index Universe, a digital asset must meet the following criteria as of the Reconstitution Reference Date:

1.  The digital asset must be included in the list of 250 digital assets as published in the latest DACS.
2.  A CoinDesk reference rate (“Reference Rate”) must exist for the digital asset with a minimum of two contributing exchanges (refer to the CoinDesk Reference Rate Methodology for more details).
3.  Meme coins and digital assets defined as securities by a U.S. government oversight agency are eligible for inclusion.
4.  See Table 2 for additional eligibility requirements for each individual index.

The Indices are determined based on DACS screens from the Index Universe.

### Table 2: Additional Eligibility Requirements

| Index Name                               | Additional Index Eligibility Requirements                |
| ---------------------------------------- | -------------------------------------------------------- |
| CoinDesk Market Index                    | Digital assets not classified in the Stablecoin Sector   |
| CoinDesk Market Plus Stablecoin Index    | No additional eligibility requirements                   |
| CoinDesk Computing Index                 | Digital assets classified in the Computing Sector.       |
| CoinDesk Culture & Entertainment Index   | Digital assets classified in the Culture & Entertainment Sector |
| CoinDesk Currency Index                  | Digital assets classified in the Currency Sector         |
| CoinDesk DeFi Index                      | Digital assets classified in the DeFi Sector             |
| CoinDesk Digitization Index              | Digital assets classified in the Digitization Sector     |
| CoinDesk Smart Contract Platform Index   | Digital assets classified in the Smart Contract Platform Sector |
| CoinDesk Stablecoin Index                | Digital assets classified in the Stablecoin Sector       |

## Index Construction

### Constituent selection

All digital assets that pass the Eligibility Criteria are selected for index inclusion.

### Constituent weighting

Constituents are weighted based on market capitalization.

### Circulating supply

At each reconstitution, the circulating supply for each constituent is determined based on the latest circulating supply on the Weighting Reference Date (see Index Maintenance). The circulating supply is not updated between reconstitutions.

### Index calculation

The indices are calculated in real-time using Reference Rates for each underlying constituent as described below.

### Constituent pricing

Reference Rates for constituent digital assets are calculated approximately every 5 seconds by CDI using a volume weighted average price (VWAP) across three contributing exchanges over the prior 60 minutes. These reference rates are referred to as Settlement Reference Rates in the CoinDesk Reference Rate Methodology.

The number of contributing exchanges may drop below two between reconstitutions. In the event the number of contributing exchanges drops below two between reconstitutions, the Index Committee will review the eligible exchanges at the end of the month to determine the appropriate course of action.

### Calculation Formula

The Index is calculated using the following formula:

\[
Index_t = \frac{\sum_{i=1}^{N} P_{i,t} \times S_i \times WAF_i}{Divisor}
\]

where,

-   \(Index_t\) is the value of the Index at time t
-   \(P_{i,t}\) is the price of constituent i at time t, as determined by its Settlement Reference Rate,
-   \(S_i\) is the circulating supply of constituent i as of the Weighting Reference Date,
-   \(WAF_i^1\) is the Weighting Adjustment Factor of constituent i, as of the Weighting Reference Date,
-   \(Divisor\) is the Index Divisor.

### Index Divisor Adjustment

The Index Divisor is recalculated on each reconstitution Effective Date and during any event which requires a change to the index constituents not driven solely by market price movements.

\[
Divisor_{NC} = Divisor_{PC} \times \frac{\sum P_{i,SETT} \times S_{i,NC} \times WAF_{i,NC}}{\sum P_{i,SETT} \times S_{i,PC} \times WAF_{i,PC}}
\]

where,

-   \(P_{i,SETT}\) is the price of constituent i as of the Effective Date, or other date and time at which the Divisor Adjustment takes place, determined by its Settlement Reference Rate,
-   \(NC\) represents the respective values of the new index constituents following the application of all changes and \(PC\) represents the respective values of the prior index constituents. Note that the price used in the numerator and denominator for each constituent is based on the settlement reference rates as of the effective timestamp of the changes.

## Index Maintenance

### Index reconstitution

Each Index is reviewed quarterly based on the rules described above. Index changes resulting from the quarterly reconstitution are announced two weeks prior to the effective date and implemented on the second business day of the following month. Reconstitution effective dates are the second business day of January, April, July and October.

### Table 4: Reconstitution Timing

| Activity Description                      | Timing                                                 | Example Reconstitution Timing |
| ----------------------------------------- | ------------------------------------------------------ | ----------------------------- |
| Reconstitution Reference Date             | Announcement Date minus 2 business days                | 06/14/2024                    |
| Announcement Date                         | Effective Date minus 14 calendar days (following business day if this is a holiday) | 06/18/2024                    |
| Lockdown Circulating Supply (Weighting Reference Date) | Effective Date minus 7 calendar days (following business day if this is a holiday) | 06/25/2024                    |
| Effective Date                            | 4PM Eastern Time on the 2nd business day of January, April, July and October | 07/02/2024                    |

¹ Indices in this methodology are weighted by market capitalization and, therefore, Weighting Adjustment Factors are set to 1.0.

In addition to the quarterly reconstitution, constituents are monitored for potential anomalies and trading disruptions. Out-of-review monitoring, which would require an Index modification, only applies in extraordinary circumstances. Incident types that would require one or more Index modifications are outlined in the CoinDesk Digital Asset Indices Policy Methodology.

### Deletions

If a constituent is removed from an Index outside of the scheduled reconstitution process, its weight will be redistributed proportionally to all remaining constituents as of the effective date of removal. The impacted constituent will be considered for inclusion at future reconstitutions if it meets the Eligibility and Selection Criteria.

### Additions

There will be no additions to the Index between reconstitutions.

### DACS reclassifications

In the event a constituent’s DACS classification is modified between reconstitutions, no changes will be made to its Index membership until the next reconstitution.

## Data Distribution

Real-time Index values are calculated 24x7 at 5 second intervals and are available publicly at https://coindesk.com/indices/ and are also available to subscribers via REST, WebSocket APIs, and scheduled email updates.

## Index Governance

The CoinDesk Index Committee provides ongoing oversight of the Index and its Methodology. For more details on the Index Committee, please refer to the Index Governance section of the CoinDesk Digital Asset Indices Policy Methodology.

## Appendix 1: Data sources

This section describes data sources used to maintain, reconstitute, and calculate the Indices since the initial base date. For details of data sources prior to the base date, please see the CMI Family Backcast Appendix. If data is not available for any reason from the sources described in this appendix, other data sources may be used.

### Price data

Prices used to calculate Reference Rates are sourced from Eligible Exchanges.

### Volume data

Volume data used to calculate Reference Rates are sourced from Eligible Exchanges.

### Reconstitution Data

Market capitalization and circulating supply data are provided by CCData and are taken at midnight UTC. Circulating supply and market capitalization data is subject to review by CDI and may be modified for appropriateness.

Daily notional volume is sourced from eligible exchanges. Daily notional generally reflects midnight UTC but is determined by the convention that the exchange uses, which may differ. In the event an exchange does not explicitly provide daily notional, CDI will estimate daily notional for a digital asset using the following formula:

`Daily Notional Volume = Daily trading volume * (open price + close price)/2`

## Appendix 2: CMI Family Backcast Methodology

### Overview

In April 2023, CDI completed a backcast of the CoinDesk Market Index family to extend history prior to the launch date of August 29, 2022 for all indices except USCE. History for the indices was extended as far back as December 29, 2017 if constituents were available. Table 1 provides the first value date for each index that was backcasted. The sections below provide a summary of the methodology used to backcast the indices.

### Eligibility Requirements

To be eligible for the backcast, a digital asset must meet the following requirements:

-   Be a constituent of CMIP as of August 29, 2022.
-   Have readily available prices in Amberdata’s historical spot price endpoint for the USD pair endpoint as of the reconstitution reference date.
-   Have circulating supply available from Amberdata’s historical supply endpoints of the reconstitution reference rate.

See Table 2 for additional eligibility requirements for each individual CMI Index.

### Selection

Digital assets that meet all eligibility requirements were included in the index.

### Constituent Pricing

Digital asset pricing was sourced from Amberdata’s historical spot price pairs endpoint. For more information on Amberdata’s pricing methodology, please visit https://www.amberdata.io/.

### Circulating Supply

Circulating Supply was sourced from Amberdata’s historical supply endpoint and updated monthly. Supplies were pulled from the last calendar day of each month and implemented in the following month’s reconstitution. For more information on Amberdata’s circulating supply methodology, please visit https://www.amberdata.io/.

### Reconstitution
Indices were reconstituted each month and implemented at 4p.m. Eastern Time on the 2nd business day of the month. The reference date for data is the last calendar day of the prior month.

### Minimum constituent count

An index will be calculated for any period where one or more constituents were selected.

### Calculation Frequency

Index levels during the backcast were calculated on an hourly basis.

### Data Integrity

Historical supply and pricing data from Amberdata were reviewed by CDI’s research and operations teams to identify and resolve gaps or inconsistencies in circulating supply and price data.

### Initial Inclusion Date

For each eligible digital asset, CDI assigned an initial inclusion date for the backcast based on the following steps:

Step1: Determine the latest date during the backcast period where the digital asset experienced no extended gaps in pricing and had a valid circulating supply.
Step 2: Assign the initial inclusion date for the digital asset as the reconstitution date immediately following the date in Step 1 assuming it met all other eligibility and selection requirements.

## Appendix 3: Methodology Changes

The table below is a summary of modifications to this Methodology.

| Effective Date | Prior Treatment                                                                                                                                                                                                                         | Updated Treatment                                                                                                                                                                                                                                                                                                                           | Material Change |
| :------------- | :-------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | :------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | :-------------- |
| Dec 10, 2024   | Index calculation formula is a weighted return of each constituent’s weight as of the prior reconstitution effective date multiplied by is price return since the prior reconstitution effective date.                                   | Index is calculated by summing up the market value of each constituent and dividing by a Divisor                                                                                                                                                                                                                                            | No              |
| May 1, 2024    | CMI Indices reconstituted monthly.                                                                                                                                                                                                        | CMI Indices reconstituted quarterly in January, April, July and October..                                                                                                                                                                                                                                                                   | Yes             |
| May 1, 2024    | Reconstitution changes announced 7 calendar days prior to the implementation date.                                                                                                                                                      | Reconstitution changes announced 14 calendar days prior to the implementation date.                                                                                                                                                                                                                                                         | No              |
| Aug 23, 2023   | Universe Eligibility: The digital asset must have been listed and trading on an Eligible Exchange for a minimum of 30 calendar days leading up to and including the Reconstitution Reference Date. For details on Eligible Exchanges, refer to the CoinDesk Digital Asset Indices Policy Methodology. | This requirement was removed as it is already covered by the requirement of a CoinDesk Reference Rate                                                                                                                                                                                                                               | No              |
| Dec 21, 2022   | CoinDesk Market and CoinDesk Currency indices include stablecoins.                                                                                                                                                                      | CoinDesk Market (CMI) and CoinDesk Currency (CCY) Indices exclude stablecoins. This was achieved via ticker and names changes: Old Ticker: CMI New Ticker: CMIP Old Name: CoinDesk Market Index New Name: CoinDesk Market Plus Stablecoin Index Old Ticker: CMIX New Ticker: CMI Old Name: CoinDesk Market Ex Stablecoin Index New Name: CoinDesk Market Index Old Ticker: CCX New Ticker: CCY Old Name: CoinDesk Currency Ex Stablecoin Index New Name: CoinDesk Currency Index Legacy CCY index is discontinued, and users can point to CCY new as a replacement. | Yes             |
| Nov 2, 2022    | Weighing reference date used to determine circulating supply is two business days prior to the reconstitution effective date.                                                                                                             | Weighing reference date used to determine circulating supply is seven calendar days prior to the reconstitution effective date.                                                                                                                                                                                                           | No              |

