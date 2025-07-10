# Coindesk Memecoin Index Methodology

## Introduction

### Index Objective
The CoinDesk Memecoin Index (“the Index”) measures the performance of a selection of digital assets identified as memecoins. Index constituents are ranked and selected each month based on recent trading volume with a target count of 25 to 50 constituents. Constituents are equally weighted at each reconstitution.

### Additional Details
Memecoins are tokens and blockchains inspired by internet memes or popular culture, often created for fun. The Index selects from digital assets with an Asset Industry Name of “Meme” as assigned and maintained by CCData [see Appendix 1: Data Sources].

The Index is designed to capture memecoin market exposure and volatility, acknowledging memecoins’ inherently frivolous nature and lack of utility at launch. Memecoin holders and traders speculate primarily to pursue highly convex returns based on the formation of communities of buyers and other network effects. The Index includes no requirements nor safeguard with respect to the size, legitimacy, or utility of constituents.

This methodology was created by CoinDesk Indices (“CDI”) to achieve the Index Objective stated above. There may be circumstances or market events which require CDI, in its sole discretion, to deviate from these rules to ensure each Index continues to meet its Index Objective. This document should be read in conjunction with the CoinDesk Digital Asset Indices Policy Methodology (the “Policy Methodology”).

### Table 1: Index Details

| Index Name | Symbol | Target Count | Launch Date | Base Value | Base Date |
| :--- | :--- | :--- | :--- | :--- | :--- |
| CoinDesk Memecoin Index | CDMEME | Minimum 25 Maximum 50 | Dec 19, 2024 | 1000.00 | Oct 31, 2024 |

## Eligibility Criteria

### Index Universe Eligibility
To be included in the Index Universe, a digital asset must meet the following criteria as of the Reconstitution Reference Date:

1. The digital asset must be included in the “Meme” Industry Group in CCData’s metadata dataset.
2. The digital asset must be able to support an applicable Reference Rate, as defined below, with at least three contributing trading venues with non-zero volume on the Reconstitution Reference Date.
3. The digital asset must have traded in each of the 30 days leading to and including the Reconstitution Reference Date on venues captured in the applicable Reference Rate.
4. The digital asset must not present or espouse offensive themes. The Index Committee may deem such offensive memecoins ineligible.

## Index Construction

### Constituent Selection
Constituents are selected from the Index Universe using the following steps:

1. Rank all digital assets in the Index Universe by 30-day average daily value traded (ADVT) in descending order for the 30 days up to and including the Reconstitution Reference Date. Daily volume is sourced from the exchanges and trading pairs used in the applicable Reference Rates as defined below.
2. If more than 25 digital assets have an ADVT of $1,000,000 or greater, select those assets with the largest ADVT, up to a maximum of 50 constituents.
3. If fewer than 25 digital assets have an ADVT of $1,000,000 or greater, select the 25 digital assets with the largest ADVT.

### Constituent Weighting
Constituents are equal-weighted. See Index Maintenance for more information about index reweighting during reconstitutions.

Weighting Adjustment Factors are determined on the Weighting Reference Date [see Index Maintenance below] to achieve the equal weights, fixing the constituent “portfolio” in advance of the Effective Date of the reconstitution. As such, constituent weights as of the Effective Date will drift with constituent price movements between the Weighting Reference Date and the Effective Date.

## Index Calculation

### Constituent Pricing
Constituent prices are calculated using CCData CADLI (“Reference Rates”) that incorporate fiat and converted stablecoin pairs from a selection of centralized and decentralized digital asset exchanges. The methodology for these Reference Rates, which are also used in reconstitution calculations, can be found here[^1].

### Index Calculation Formula
The Index is calculated using the following formula:
\[
\text{Index}_t = \frac{\sum_{i=1}^{N} P_{i,t} \times \text{SF} \times \text{WAF}_i}{\text{Divisor}}
\]
where,

-   **Index_t** is the value of the Index at time t,
-   **P_i,t** is the price of constituent i at time t, as reflected in its Reference Rate,
-   **SF** is the Supply Factor[^2], which is equal to 1,000,000,
-   **WAF_i** is the Weighting Adjustment Factor of constituent i as of the Weighting Reference Date,
-   **Divisor** is the Index Divisor as of the Reconstitution Effective Date.

[^1]: The Reference Rates for the Index will transition to a version that excludes “Excluded Exchanges” as designated in the Policy Methodology. This change, which is expected to take place in Q1 of 2025, will be announced.

[^2]: A common characteristic of memecoins is a very small price per token, often small fractions of $.01. In order to maintain an Index Divisor with less decimal place dependency, a large fixed Supply Factor is used. Since the constituent weights of the Index do not depend on the market capitalization of the digital asset, constituent circulating supplies are not utilized.

### Divisor Adjustment
The Index Divisor is recalculated on each reconstitution Effective Date and during any extraordinary events which require a change to the index constituents.
\[
\text{Divisor}_{\text{NC}} = \text{Divisor}_{\text{PC}} \times \frac{\sum_{i=1}^{N} P_{i} \times \text{SF} \times \text{WAF}_{i}}{\sum_{j=1}^{M} P_{j} \times \text{SF} \times \text{WAF}_{j}}
\]
Where,

-   **P_i** is the price of constituent i as of the Effective Date, or other date and time at which the Divisor Adjustment takes place, determined by its Reference Rate,
-   **M** is the number of index constituents prior to the reconstitution,
-   **N** is the number of index constituents following the application of all reconstitution changes,
-   The subscript **PC** represents the respective values of the prior index constituents,
-   The subscript **NC** represents the respective values of the new index constituents following the application of all reconstitution changes.

Note that the price P used in the numerator and denominator for each constituent is based on the Reference Rates as of the effective timestamp of the changes.

## Index Maintenance

### Index Reconstitution
The Index is reviewed and reconstituted on a monthly basis based on the rules described above. There are two forms of reconstitution, full and partial:

Full reconstitutions include reselection of index constituents and application of equal weighting. Full reconstitutions are conducted quarterly and scheduled so that their Effective Dates fall on the last business day of January, April, July and October.

Partial reconstitutions include reselection of index constituents. Entering constituents are assigned a weight equal to the inverse of the number of index constituents at the conclusion of the partial reconstitution. Excess weight–positive or negative–is distributed proportionately among other constituents[^3]. Partial reconstitutions are conducted during months when full reconstitutions do not take place and are scheduled so that their Effective Dates fall on the last business day of each such month.

Reconstitutions, both full and partial, comprise four events, defined as follows:

1.  **Reconstitution Reference Date.** Snapshot date for data used to select Index constituents. This falls two business days before the Announcement Date.
2.  **Announcement Date.** The date on which changes to Index constituents are announced. This falls fourteen calendar days before the Effective Date, or the closest following business day.
3.  **Weighting Reference Date.** The date on which the Index weights and Weighting Adjustment Factors are calculated, as defined above [see Index Construction]. This falls seven calendar days before the Effective Date, or the closest following business day.
4.  **Effective Date.** 4 p.m. Eastern Time on the date on which the reconstitution becomes effective.

[^3]: New constituents will be assigned a Weighting Adjustment Factor (WAF) to achieve the target weight. Retained constituents maintain their existing WAF.

### Table 2: Reconstitution Calendar Example (full reconstitution shown here)

| Activity | Timing | Example |
| :--- | :--- | :--- |
| Reconstitution Reference Date | 2 business days prior to the Announcement Date | Jan 15, 2025 |
| Announcement Date | 14 calendar days prior to the Effective Date, or the following business day if this is a holiday | Jan 17, 2025 |
| Weighting Reference Date | 7 calendar days prior to the Effective Date, or the following business day if this is a holiday | Jan 24, 2025 |
| Effective Date | 4pm Eastern Time time on the final business day of January, April, July, and October | Jan 31, 2025 |

### Index Changes Between Reconstitutions
In addition to the monthly process, constituents are monitored for potential anomalies and trading disruptions. Out-of-review monitoring, which would require an index modification, only applies in extraordinary circumstances. Incident types that would require one or more index modifications are outlined in the Policy Methodology.

#### Additions
There will be no additions to the index between reconstitutions.

#### Deletions
If a constituent is removed from the Index outside of the scheduled reconstitution process it will not be replaced and, therefore, the constituent count may drop below the lower target number. The weight of the constituent being removed will be redistributed proportionally to all remaining constituents as of the effective date of removal. No other reweighting will be performed. The impacted constituent will be considered for inclusion at future reconstitutions if it meets the criteria described above.

### Data Distribution
Index values are calculated 24x7 and are available publicly at coindesk.com and are also available to subscribers via REST and WebSocket APIs.

### Index Governance
The CoinDesk Index Committee provides ongoing oversight of the Index and its Methodology. For more details on the Index Committee, please refer to the Index Governance section of the CoinDesk Digital Asset Indices Policy Methodology.

## Appendix 1: Data sources
This section describes data sources used to maintain, reconstitute, and calculate the Index since the initial base date. If data are not available for any reason from the sources described in this appendix, other data sources may be used.

### Memecoin classification
Memecoin classification is conducted by CCData. The process occurs continuously and includes scans of new token listings, artificial intelligence tools, industry standards, and scheduled manual reviews. The current classification can be found at the following endpoint: <https://developers.ccdata.io/documentation/data-api/asset_v1_top_list?groups=CLASSIFICATION>

### Memecoin chain and DEX support
For a complete list of the blockchains and DEX’s currently supported and included in the Index Universe, visit the following endpoint: <https://developers.ccdata.io/documentation/data-api/on_chain>. As an example, as of the launch date of the Index, Solana-based DEX’s are not supported.

### Reconstitution data
Daily volume data for liquidity analyses is sourced from CCData as of midnight UTC. Volume, pricing, classifications and reference data used for reconstitutions are reviewed by CoinDesk Indices analysts for accuracy and reliability. Based on these reviews, CoinDesk Indices reserves the right to update reconstitution data.

### Pricing Data
Pricing for constituents used to calculate the index is sourced from underlying exchanges and trading venues, including decentralized exchanges (DEX’s).

## Appendix 2: Methodology Changes
The table below is a summary of modifications to this Methodology.

| Effective Date | Prior Treatment | Updated Treatment | Material Change |
| :--- | :--- | :--- | :--- |
| May 2025 Reconstitution | Eligibility Criteria: The digital asset must have launched at least 90 days prior to be eligible. In cases where an asset’s launch date is unavailable or ambiguous, the requirement will be that the asset has a Reference Rate published for at least 90 days prior to the Reference Date. | <Criteria removed> | Yes |
| April 2025 Reconstitution | No requirement for multiple trading venues for a CADLI Reference Rate | CADLI Reference Rate must have at least 3 contributing venues with non-zero volume on the Reconstitution Reference Date | No |