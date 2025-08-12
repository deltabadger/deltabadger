# CoinDesk Digital Asset Indices Policy Methodology 
_February 2025_

## Table of Contents
- [CoinDesk Digital Asset Indices Policy Methodology](#coindesk-digital-asset-indices-policy-methodology)
- [Security Designation](#security-designation)
- [Exchange Eligibility](#exchange-eligibility)
- [Input Pricing](#input-pricing)
  - [Exchange Sourced](#exchange-sourced)
  - [Reference Rate Sourced](#reference-rate-sourced)
- [Digital Asset Maintenance](#digital-asset-maintenance)
  - [Circulating Supply](#circulating-supply)
  - [Airdrops](#airdrops)
  - [Digital Asset Removals](#digital-asset-removals)
- [Index Calculations](#index-calculations)
  - [Data Monitoring](#data-monitoring)
- [Holidays](#holidays)
- [Announcement Schedule for Index Changes](#announcement-schedule-for-index-changes)
- [Discretion](#discretion)
- [Expert Judgement (Pricing)](#expert-judgement-pricing)
- [Errors & Recalculations](#errors--recalculations)
- [Methodology Reviews](#methodology-reviews)
  - [Methodology Modifications](#methodology-modifications)
- [Consultations](#consultations)
- [Index Termination](#index-termination)
- [Blockchain Forks](#blockchain-forks)
  - [Background](#background)
  - [Index Impact](#index-impact)
  - [Hard Fork](#hard-fork)
  - [Soft Fork](#soft-fork)
- [Index Governance](#index-governance)
- [Appendix 1: Eligible Exchanges](#appendix-1-eligible-exchanges)
- [Appendix 2: Methodology Changes](#appendix-2-methodology-changes)

---

## Introduction 

CoinDesk Indices (“CDI”) is an independent index provider that maintains a series of indices designed to measure the performance of various segments of the digital asset market as described in each Index Methodology. Factors that are critical to the administration of each CoinDesk Index (the “Indices”) include maintaining a robust index design and calculation platform, appropriate monitoring and quality control tools, strong oversight, and transparent documentation.
This document is designed to provide general policies that apply across indices and should be used in conjunction with the applicable Index Methodology. This document will be updated as necessary and reviewed on an annual basis. As there may be exceptions to these policies for a specific index, the rules and procedures described in the Index Methodology supersede this document.
A digital asset is a non-tangible asset that is created, traded, and stored in a digital format using distributed ledger or blockchain technology. Digital assets are often referred to as crypto assets, cryptocurrencies, or digital tokens, among other terminologies.
A memecoin is a digital asset that is associated with a meme or viral online joke.
Typically, memecoins have no stated economic or financial purpose.

## Security Designation 

CDI’s Indices are primarily intended to be replicable by investors, particularly from the perspective of a U.S.-based investor given the utilization of USD pricing pairs, which predominantly trade in the U.S. currently.
Digital assets that are determined to be unregistered securities under the U.S. securities laws may be delisted by Eligible Exchanges and not available for investment by U.S.
persons. Accordingly, this Policy Methodology seeks to exclude such digital assets
(“Excluded Digital Assets”) from most CDI Indices. CDI may solicit feedback from stakeholders in certain circumstances. Exceptions may exist for certain indices, such as the CoinDesk Market Indices (CMI) Family, which do not have “Excluded Digital Assets”
as part of the eligibility criteria and so are not impacted by a digital asset being designated as a security.
Digital assets that have been alleged to be securities by regulatory authorities but whose status is subject to dispute will generally remain eligible constituents of the Indices to the extent they continue to meet all other eligibility criteria in the applicable index methodology.
Any determination under this Methodology with respect to whether a digital asset is (or is not) an Excluded Digital Asset should in no way be construed as a determination as to whether or not such digital asset is (or is not) a ‘security’ for legal purposes.

## Exchange Eligibility 

The criteria listed below are used to identify eligible exchanges for the purpose of calculating Single Digital Asset Price Indices and Reference Rates. To be eligible, an exchange must meet the following standards:
- No evidence in the past 12 months of trading restrictions on individuals or entities that would otherwise meet the exchange’s eligibility requirements to trade 
- No evidence in the past 12 months of undisclosed restrictions on deposits or withdrawals from user accounts 
- Real-time price discovery 
- Limited or no capital controls 
- Transparent ownership including a publicly known ownership entity 
- Publicly available language and policies addressing legal and regulatory compliance, including KYC (Know Your Customer), AML (Anti-Money Laundering) and other policies designed to comply with relevant regulations that might apply to it 
- Offer programmatic spot trading of the trading pair, and reliably publish trade prices and volumes on a real-time basis through Rest and Websocket APIs 

All exchanges that meet these Eligibility Criteria will be assigned to one Exchange Category as defined by the additional criteria below:
- **Category 1**
  - Licensed and/or able to serve investors, retail or professional, in the United States
  - Maintain sufficient USD or USDC liquidity relative to the size of the listed assets 
- **Category 2**
  - Licensed (including in-principal licensure) and/or able to serve investors, retail or professional, in one or more of the following jurisdictions:
    - United Kingdom 
    - European Union¹
    - Hong Kong
    - Singapore
  - Maintain sufficient USD or USDC liquidity relative to the size of the listed assets.

Please see Appendix 1 for a list of current Eligible Exchanges by category.
CDI reviews exchanges on a semi-annual basis to ensure current Eligible Exchanges continue to meet the requirements listed above as well as reviewing other exchanges for possible addition to the list of Eligible Exchanges. Any update(s) to the list of Eligible Exchanges will be announced in advance of an exchange being added/removed as a contributing exchange to one or more of CDI’s Single Digital Asset Price Indices or Reference Rates.
CDI regularly monitors each Eligible Exchange to ensure it continues to meet the eligibility criteria defined above. In the event CDI becomes aware of a situation that impacts an exchange's ability to meet the eligible criteria as stated above, the Index Committee will review the circumstances which may lead to the impacted exchange being removed from the list of Eligible Exchanges.
Eligible Exchanges that have met the above standards from the CDI reviews, in particular with the applicable legal and regulatory compliance in the US, where there have been allegation(s) of non-compliance but whose status is subject to dispute, will generally remain an eligible exchange to the extent they continue to meet all other eligibility exchange standards.
It is important to note that whether or not a particular exchange included in CDI’s Eligible Exchange for index calculation purposes is not, and should in no way to be construed to be, a determination as to whether or not such eligible exchange is or is not compliant with the above standards for legal purposes.

**Excluded Exchanges.** Certain CoinDesk Indices products support derivatives and other exchange-listed financial products that may require additional cooperation and support from contributing exchanges. The Benchmark Oversight Committee may designate one or more exchanges as an Excluded Exchange as it deems necessary or prudent to ensure adequate support. Excluded Exchanges will be ineligible to contribute to relevant Single Digital Asset Price Indices and Reference Rates as specified in their respective methodologies.

---
¹ In the event an exchange is only licensed or able to serve investors in select European Union countries and none of the other listed jurisdictions, CoinDesk Indices reserves the right to evaluate its eligibility on a case-by-case basis.

## Input Pricing 

This section provides an overview of the Hierarchy of Data Inputs for pricing.

### Exchange Sourced 
CDI calculates Single Digital Asset Price Indices and Reference Rates to provide a benchmark price for certain single digital assets. Both Single Digital Asset Price Indices and Reference Rates leverage real-time spot prices from eligible exchanges as inputs to index calculations. Spot prices in this context refer to executed trades, which occur between market participants on digital currency exchanges. This definition includes transactions in which market participants buy or sell digital currencies on exchanges for fiat or other digital currencies with an immediate settlement. Spot prices are differentiated from forward, futures, options, and swaps prices which represent the transactional price that derivative contracts can be bought or sold at for delivery at a future date and with different transactional requirements.
Before they are utilized to calculate Single Digital Asset Price Indices and Reference Rates, real-time spot prices are ingested and normalized from eligible exchange APIs.

### Reference Rate Sourced 
For indices with multiple digital asset constituents, input prices are sourced from underlying CoinDesk Reference Rates.

## Digital Asset Maintenance 
### Circulating Supply 
CoinDesk Indices leverages circulating supply data, the intent of which is to represent the total number of coins that are publicly available. See CoinMarketCap for more details on how they determine circulating supply.
### Airdrops 
There are generally no adjustments to CoinDesk Indices for airdrops. Airdrops are events that involve sending free tokens to members of their communities in an effort to encourage adoption.
### Digital Asset Removals 
Digital assets that are removed from a CoinDesk Index will be removed at the last calculated Reference Rate at the time of removal. In the unlikely event a digital asset is no longer listed or available to trade on at least one of the Eligible Exchanges included in the Reference Rate calculation, CoinDesk Indices reserves the right to use an alternate price to accurately reflect the current value of the token. (For example, if a digital asset is suspended from trading on current contributing exchanges but is actively pricing on another trading venue.) In the absence of a tradable price, CoinDesk Indices may remove the asset at a price of zero.
In situations where CDI does not use the last calculated Reference Rate to remove an asset, an announcement to users will be provided.

## Index Calculations 
Indices that are calculated in real-time rely on streaming input pricing as described above. Delays or disruptions in receiving input pricing may impact real-time index levels.
Real-time indices, and associated Reference Rates, are calculated on an interval basis
(e.g. every 1 or 5 seconds) as detailed in the applicable Index Methodology. Since the indices are calculated on an interval, they do not capture every possible index level between calculation cycles. For this reason, real-time indices may not reflect the full range of price movements for a given trading day.
### Data Monitoring 
CDI designs and maintains the required infrastructure to support operation of its indices by ensuring replicability, availability, and performance for price calculation and distribution.
CDI has a robust monitoring process of its systems and data input in place that ensures the health of inbound data feeds, calculations, and distribution of index levels. In the event issues are detected during the monitoring process, employees are notified so that they focus on analyzing and, if necessary, addressing the issue.

## Holidays 
Calculation of CoinDesk Indices is not impacted by holidays. Indices are calculated on each day of the year.
Index reconstitutions and rebalancing schedules are outlined in each Index Methodology and are based on business days. Business days are defined as weekdays that are not classified as holidays. CoinDesk Indices follows the U.S. Bank Holiday schedule to determine holidays.

## Announcement Schedule for Index Changes 
Index changes are events that impact the constituents, methodology, timing of reconstitutions, list of Eligible Exchanges, extraordinary situations or other actions that affect the administration of a CoinDesk Index or Reference Rate. CDI identifies three categories of user announcements when there is an index change, either scheduled or unscheduled. If applicable, announcements are communicated to users and posted on the CDI website under the Index Governance page found here.
Index announcements fall under one of the following three schedules:
- **Type A:**
  - Scheduled announcement 
  - User notice communicated prior to implementation 
  - Implementation timing is detailed in the respective index methodology 
  - Index change due to scheduled index review
- **Type B:**
  - Unscheduled announcement 
  - User notice communicated prior to implementation 
  - Implementation at a subjective time after user notice is sent 
  - Index change due to routine monitoring
- **Type C:**
  - Unscheduled announcement 
  - Advanced notification may not be possible 
  - Immediate implementation 
  - Index change is due to index mechanism being compromised 

Type A: Index announcements are initiated and implemented during regularly scheduled index reviews as described in the applicable index methodology.
Type B: Indices and constituents are routinely monitored for possible issues and disruptions. In certain circumstances, CDI may determine that index changes may need to be implemented prior to the next scheduled review to minimize the impact on the index and ensure the Index continues to meet its stated objective. For example, if a constituent exchange begins the delisting process for a trading pair included in the index, CDI may determine to remove the constituent exchange. In this case, CDI will provide notification to index users of the exchange removal. Given the nature of these types of events, the timing between announcement and implementation for these types of changes will vary based on the specific situation.
Type C: Certain incidents may require immediate implementation from CDI. As such,
index users are not informed ahead of time regarding Type C announcements to the index but will still be provided with an announcement either when the change occurs or immediately after, where possible. Examples of Type C announcements include an exchange restricting withdrawals or exchange insolvency.
If one of the example events is uncovered during routine monitoring, CDI will immediately take action to remove the constituent from the index and send an index announcement to index users to alert them to the change.

## Discretion 
CDI’s Index and Policy Methodologies are designed to provide transparency regarding its approach and rules for managing and calculating indices. There may be circumstances based on market conditions or other factors that may require the Index Committee to deviate from its Methodology and/or determine an appropriate course of action based on situations not contemplated in its Methodology to ensure the successful administration of an index.
CDI believes that limiting turnover, when appropriate, may improve the ability to replicate an index. This may result in a constituent continuing to be included in an index that otherwise would not meet the index rules and/or an eligible digital asset being excluded that otherwise meets the rules. Generally, these situations would be limited to borderline cases.

## Expert Judgement (Pricing)
CDI has established algorithms to determine real-time volume weighted prices and constituent weighting adjustments for certain indices. These processes leverage exchange sourced data and no input is provided by analysts and, therefore, no expert judgement is being applied.

## Errors & Recalculations 
The following section provides general guidelines for the handling of errors and recalculations based on certain scenarios that may arise during the administration of an index or Reference Rate. The Index Committee reserves the right to deviate from these guidelines. Any decision to recalculate an index will be announced to subscribers.

| Scenario | Action |
| --- | --- |
| Input Errors | If CDI becomes aware of an incorrect exchange price or transaction volume, impacted indices and/or Reference Rates will generally not be recalculated. |
| Errors by CDI | Errors and omissions caused by CDI analysts or technology will generally be corrected if discovered within two business days. Errors and omissions discovered beyond two business days will be reviewed by the Index Committee. |
| Misapplication of Index Methodology or Incorrect Announcement | In the event an error is discovered that is related to the application of the Index Methodology or Announcement of Changes, the Index Committee will review the event to determine the appropriate course of action. |
| Implementation Error | In the event CDI becomes aware of the incorrect implementation of previously announced changes, CDI will generally correct the error and recalculate impacted index data if the error is detected within one-week of the implementation date. |
| Errors in Point-in-Time Data for Index Rebalancing Events | CDI relies on Point-in-Time data to determine eligible digital assets as well as select and/or weight constituents. The data used to source these data are generally included in the applicable Index Methodology. In the event there were errors in a set of Point-in-Time data sourced from a third party, CDI will not recalculate an index, post implementation. In the event the error is discovered prior to implementation, CDI Indices will review the impact of the error as well as the timing of implementation to help guide its decision. |

## Methodology Reviews 
Index Methodologies are reviewed on an annual basis to ensure each index continues to meet its stated objective. The review also aims to ensure that the rules to manage,
rebalance and calculate each index are accurate, transparent, and complete.

### Methodology Modifications 
Index Methodologies may require modifications and are bucketed into 3 types:
- **Type 1:** Clarifications to Index Methodologies which increase transparency around a rule or process by eliminating ambiguity and/or providing additional details.
- **Type 2:** Editorial updates which are intended to update standard language or disclaimers, correct grammatical issues, or simplify documents.
- **Type 3:** Material modifications to an index objective or rules used to manage,
calculate, or reconstitute an index (see Consultations section).

Type 1 and 2 modifications are not subject to advance notice and will be incorporated in the latest version of the Index Methodology published on the CDI website.
In the event a Type 3 modification is made to an index, subscribers will be provided reasonable advance notice based on the impact of the modification. The notification will include the modification to the Index Methodology and the effective date. These changes will be incorporated into the latest version of the Index Methodology published on the CDI website and a new entry will be added to the Appendix which details material methodology changes.
Changes to the source or timing of input data used during the application of the Index Methodology will generally not be considered as Type 3 changes but rather a Type B or C announcement to users - refer to announcement section. An exception to this rule may include situations where the new data source results in materially different results than the current source.

## Consultations 
CDI may seek feedback from subscribers and interested parties through a public consultation process. In general, consultations will be issued when material changes to the index objective or rules are contemplated but may also be issued to seek feedback on market trends, regulatory or governance concerns, upstream events that may have an impact on the index, or other matters where feedback from the user community would be helpful for the Index Committee. Consultations may include one or more proposals with supporting analysis and/or open-ended questions seeking feedback from market participants.
Consultations will be opened for a specified comment period (generally 2 to 4 weeks)
that gives market participants time to analyze and respond to the consultation. During this period, CDI may provide additional analysis or details based on incoming feedback and/or requests. CDI may decide to extend the consultation period, if necessary. CDI may engage in dialogues with interested parties but will rely on written responses to the consultation as the primary source of feedback.
Following the comment period, feedback will be gathered and reviewed by the Index Committee to determine an appropriate course of action. Any material changes will be treated as a Type A Announcement as described in the Index Methodology Modifications section.
Prior to the implementation of consultation results, the Index Committee reserves the right to apply certain aspects of the Consultation in its current practices to avoid unnecessary turnover. For example, if the consultation is seeking feedback to exclude certain types of digital assets, this rule may be temporarily adopted at an upcoming reconstitution if a decision is still pending. This temporary measure will be announced to subscribers as part of the Consultation.
Consultation feedback is available upon request.

## Index Termination 
CDI reserves the right to terminate an existing index based on certain conditions. While not an exhaustive list, here are the primary reasons used for terminating an index:
1. Material changes in the underlying market that prevent the index from meeting its stated objective.
2. Regulatory or other structural changes that prohibit CDI from maintaining,
rebalancing, and/or calculating the index.
3. Inability of CDI to continue sourcing the necessary inputs to maintain, rebalance or calculate the index.
4. Lack of market adoption/usage.
CDI will coordinate with Stakeholders and, if necessary, Regulatory Authorities to identify possible alternatives or modifications that may prevent an Index Termination.
Whenever possible, CDI will provide advance notice of a scheduled Index Termination to its index subscribers. In cases when scheduled index reconstitutions or rebalances occur between the announcement of its termination and the effective date of the termination, CDI will generally skip these events.

## Blockchain Forks 
### Background 
Digital currencies, unlike traditional financial assets, can undergo ‘forks’ which may or may not result in two independent currencies, as a consequence of a disruption in the existing blockchain. The blockchain and corresponding digital currency are distinct from each other. A blockchain is a chain of hashed inputs, continually referencing its predecessor, while a digital currency is a chain of digital signatures, with an implied intrinsic value.
There is a handful of unique protocols in the digital currency space, with the majority of digital currencies existing on one of these unique protocols. Forks are divided into two categories: hard and soft forks.
A hard fork is a change to the fundamental structure of integral consensus units of a given protocol. Nodes are participants that assume the responsibility of propagating transactions and maintaining a historical account of the protocol, according to consensus standards. In a blockchain ecosystem, there exists two node types that have the functionality of endorsing consensus. The two nodes are full nodes, also referred to as archival nodes, and mining nodes.
A full node contains a full blockchain database to verify transactions, without an external reference; it routes messages over a peer-to-peer (P2P) network. A mining node competes to create new blocks, according to protocol consensus. If nodes do not adhere to protocol changes, in the event of a fundamental change to integral consensus units, they can become disenfranchised from the adjacent ecosystem. As a result, this node(s) has forked from its former peer nodes.
Notable examples of hard forks resulting in distinct digital currencies include Bitcoin Cash forking from Bitcoin and Bitcoin Satoshi’s Vision (SV) forking from Bitcoin Cash
(now denominated Bitcoin Cash Adjustable Blocksize Cap). The Bitcoin SV hard fork from Bitcoin Cash occurred at block height 556,767 - timestamped November 15th,
2018, at 06:16 UTC on the Bitcoin Cash SV chain. Users and miners initially formulated divergent definitions of consensus for Bitcoin Cash. Subsequently, each side formalized and then implemented at the fork block height, respective consensus adjustments resulting in the digital currencies referenced as BCH (Bitcoin Cash ABC) and BSV (Bitcoin Cash SV).
A soft fork is a backwards compatible software change which can be initiated by either miner, miner-activated soft fork (MASF) - or users - user-activated soft fork (UASF).
MASF is the instance for which miners preemptively signal to mimic coordination then propagate transactions according to the new standards they wish to comply with.
UASF occurs at a specified date and is enforced by participation from full nodes. Given the number of participants required in order to implement a UASF, significant industry support and coordination is needed between requisite parties.
However, if users choose not to run a codebase pushed as a result of MASF or miners do not choose to contribute to new rules set forth from a UASF, then what was initially a soft fork becomes a contentious hard fork. If a soft fork does occur, there is not a creation of a new digital currency.
### Index Impact 
#### Hard Fork 
The result of a Digital Currency hard fork is more than one competing medium of exchange.
CDI employs a reliable, transparent, and efficient governance model, which extends to managing contentious forks. Index price feeds are reliant on data and practices sourced from constituent exchanges. CDI follows precedent on a per case basis established, by the existing digital currency ecosystem.
Managing forked coins results in an unscheduled modification and follows Type B Announcement as defined in the Announcement Schedule for Index Changes section.
CDI intends to notify users prior to implementation of the required adjustment; however,
this will take place within the time frame the exchange in question notifies users of which pairs it will list post-fork. In addition to using index constituent exchanges as a guide for navigating pair contributions to indices, CDI will actively monitor network effects to grasp an understanding of each digital asset’s future utility and value.
The fork leading to the creation of Bitcoin Cash serves as an example of CDI’s policy on managing forks as constituents for an index. Bitcoin Cash forked from Bitcoin at block height 478559 - timestamped August 08, 2017, at 18:12 UTC on the Bitcoin Cash chain.
Referencing precedence, CDI (at the time, TradeBlock) did not engage in an unscheduled modification - responding to a blockchain fork may be a Type B or Type C Announcement - for the XBX Index, leading to the aforementioned fork, rather monitored the situation. The constituent exchanges for the CoinDesk XBX Index, at the time of the fork, were: Bitstamp, Coinbase, itBit, Kraken, and Okcoin. A majority of the constituent exchanges signaled their intent to denominate Bitcoin Cash as a new quote pair: BCH. In addition, the broader ecosystem expressed a desire to follow these designations. As a result, CDI (at the time, TradeBlock) did not make any modifications to the XBX index.
In the event a constituent exchange denominates the minority forked chain a misleading quote pair, CDI will remove the exchange as a constituent. Timing of the unscheduled modification is reliant on announcements the exchange shares regarding its fork maintenance policies. If the constituent exchange posts an announcement regarding fork maintenance policy prior to the chain fork, CDI will follow a Type B Announcement schedule. Conversely, if the constituent exchange posts an announcement regarding fork maintenance policy after the chain fork, CDI will follow a Type C Announcement schedule. If an exchange offers no indication of its intentions, regarding a blockchain fork, CDI will assume a Type C Announcement schedule if a change to index constituents is necessary.
#### Soft Fork 
There is no impact to an index as a result of a soft fork.

## CoinDesk Index Advisory Council 
The CoinDesk Index Advisory Council is composed of key digital asset market participants and other influential individuals within the digital asset space and is leveraged on a periodic basis to discuss potential changes to its CoinDesk’s Index Methodologies.
The CoinDesk Index Advisory Council meetings are generally held on an annual basis.
While potential changes are discussed through this process, all feedback received is non-binding and all final decisions on index methodologies are made by CoinDesk Indices via its internal Index Committee. CoinDesk Indices will publicly announce changes to its index methodologies, if any, with appropriate lead time to ensure all market participants have access to this information at the same time.

## Index Governance 
Each CoinDesk Index and Reference Rate is governed by the CDI Index Committee.
The Index Committee provides ongoing oversight of the indices. The Index Committee meets on a periodic basis and is primarily responsible for the following functions:
1. Ownership, maintenance, and regular reviews of the Index Methodology.
2. Review and approval of material changes to the Index Methodology.
3. Review and approval of changes to the index constituents or weightings due to unscheduled reconstitutions or market disruptions.
4. Determine the impact of market events on the application of the Index Methodology.
5. Use of Discretion or Expert Judgement during the application of the Index Methodology.
6. Mitigate conflicts of interest by ensuring decisions and announcements are aligned with CoinDesk Indices' Methodologies and internal procedures.
The Index Committee periodically reports to the Benchmark Oversight Committee
(“BOC”) on its governance matters, including but not limited to client complaints, the launch of new Indices, any newly identified conflicts of interest, operational incidents
(including errors & restatements), material changes concerning the benchmarks
(including user feedback results, if any), and the results of any internal or external reviews of the benchmarks, such as audit reports.

## Appendix 1: Eligible Exchanges 
The table below provides the list of exchanges that pass the Exchange Eligibility criteria broken down by Category as defined in the Exchange Eligibility section.

| Category 1 | Category 2 |
| --- | --- |
| Bitstamp | Bitfinex |
| Coinbase | Bullish |
| Crypto.com | Bybit |
| Gemini | OKX |
| itBit | |
| Kraken | |
| LMAX Digital | |

## Appendix 2: Methodology Changes 
The table below is a summary of modifications to this Methodology.

| Effective Date | Section | Prior Treatment | Updated Treatment | Material Change |
| --- | --- | --- | --- | --- |
| 12/30/2024 | Exchange Eligibility Appendix | n/a | OKX added as an Eligible Category 2 Exchange | No |
| 11/1/2024 | Exchange Eligibility Appendix | Bitflyer and Okcoin included as Eligible Exchanges | Bitflyer and Okcoin removed as Eligible Exchanges due to lack of sufficient USD or USDC liquidity | No |
| 10/11/2024 | Exchange Eligibility Section | n/a | Added definition of Excluded Exchanges which does not impact existing indices or reference rates | No |
| 10/11/2024 | Exchange Eligibility Section | Be an exchange that is licensed and able to service investors in one or more of the following jurisdictions: United States, United Kingdom, European Union, Hong Kong, Singapore | Category 1: Licensed and/or able to serve investors, retail or professional, in the United States<br>Category 2: Licensed (including in-principal licensure) and/or able to serve investors, retail or professional, in one or more of the following jurisdictions: United Kingdom, European Union, Hong Kong, Singapore | |
| 5/1/2024 | Exchange Eligibility Appendix | Cboe Digital included as an Eligible Exchange | Cboe Digital removed as Eligible Exchange following its announcement to wind down Cboe Digital Spot Market in Q3 2024 | No |
| 4/1/2024 | Exchange Eligibility | Only U.S. licensed exchanges with USD denominated trading pairs qualify to be considered as Eligible Exchanges | U.S. and non-U.S. licensed exchanges in certain jurisdictions with USD or USDC pairs qualify to be considered as Eligible Exchanges. Appendix 1 updated with latest Eligible Exchange by Category. | Yes |
| 7/21/2023 | Security Designation | Digital assets that are securities, as defined by the federal securities laws, or under such consideration by any U.S. government oversight agency, are not eligible for inclusion in a CoinDesk index with the exception of the CoinDesk Market Index family. | CDI’s Indices are primarily intended to be replicable by investors, particularly from the perspective of a U.S.-based investor given the utilization of USD pricing pairs, which predominantly trade in the U.S. currently. Digital assets that are determined to be unregistered securities under the U.S. securities laws may be delisted by Eligible Exchanges and not available for investment by U.S. persons. Accordingly, this Policy Methodology seeks to exclude such digital assets (“Excluded Digital Assets”) from most CDI Indices. CDI may solicit feedback from stakeholders in certain circumstances. Exceptions may exist for certain indices, such as the CoinDesk Market Indices (CMI) Family, which do not have “Excluded Digital Assets” as part of the eligibility criteria and so are not impacted by a digital asset being designated as a security. | Yes |
| 7/21/2023 | Exchange Eligibility | ... | ... | No |
| 6/13/2023 | Eligible Exchange Appendix | Binance.US included as an Eligible Exchange | Binance.US removed as an Eligible Exchange | No |
| 5/26/2023 | Eligible Exchange Appendix | N/A | Inclusion of Crypto.com as Eligible Exchange | No |
| 1/11/2023 | Digital Asset Maintenance | N/A | Addition of Digital Asset Maintenance section to provide details on circulating supply, airdrops and pricing digital assets upon index removal. | No |
| 1/11/2023 | Holidays | N/A | Addition of Holidays section to define holiday treatment. | No |
| 1/11/2023 | Eligible Exchange Appendix | FTX.US included as an Eligible Exchange | FTX.US removed as an Eligible Exchange | No |
| 5/4/2022 | Eligible Exchanges Appendix | N/A | Inclusion of FTX.US as Eligible Exchange | No |