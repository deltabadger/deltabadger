# Vitalik on the return of Ethereum | The Network State Podcast
Vitalik Buterin is the co-founder of Ethereum and Zuzalu. We had a fireside chat at Network School on the state of Ethereum, defi, prediction markets, AI, biotech, stablecoins, the Abundance Agenda, San Francisco, Zuzalu, and startup societies.

This discussion was held live at Network School. Apply at https://ns.com.

OUTLINE
0:00 - The state of Ethereum
02:13 - Would the average person use DeFi?
11:01 - Prediction markets from theory to reality
12:29 - Ezra Klein and Derek Thompson's Abundance Agenda needs Ethereum
15:09 - Vitalik on AI doom
18:00 - Prompting is just higher order programming
24:05 - Physical constraints on the Skynet scenario
37:14 - Startup societies, network states, and Zuzalu
42:22 - Why crypto loves bio
52:01 - Why didn't Vitalik build Ethereum anon?
54:55 - 1000X public goods
57:12 - Stablecoins are backed by dollars. So doesn't the US win if crypto wins?
1:05:20 - How does Ethereum become great again?
1:13:00 - Is SF losing its position as the center for tech?

REFERENCES
(1) Ethereum upgrades: https://ethereum.org/en/roadmap/
(2) Ethereum's next chapter: https://blog.ethereum.org/2025/04/28/...
(3) Slowing transactions on Ethereum: https://www.bbc.com/news/technology-4...
(4) L2 scaling activity: https://l2beat.com/scaling/activity 
(5) Rollup-centric Ethereum roadmap: https://ethereum-magicians.org/t/a-ro...
(6) Scaling Ethereum L1 and L2s in 2025 and beyond: https://vitalik.eth.limo/general/2025...
(7) L2 roadmap: https://ethereum-magicians.org/t/a-si... 
(8) Ethereum developers propose 150 million gas limit increase: https://www.ainvest.com/news/ethereum...
(9) What is zkEVM: https://decrypt.co/resources/what-is-...
(10) Social recovery explained: https://vitalik.eth.limo/general/2021...
(11) Multisig explained: https://vitalik.eth.limo/general/2021...
(12) Account abstraction explained: https://blog.pantherprotocol.io/ether...
(13) Parity hack: https://www.theguardian.com/technolog...
(14) Stablecoin transaction volume: https://blog.cex.io/ecosystem/stablec...
(15) What prediction markets saw: https://edition.cnn.com/2024/11/08/bu...
(16) D/acc: https://vitalik.eth.limo/general/2025...
(17) P(doom): https://www.fastcompany.com/90994526/...
(18) DeepSeek r1:   / deepseek-r1-explained-chain-of-thought-rei...  
(19) AI 2027: https://ai-2027.com/
(20) Zuzalu: https://www.zuzalu.city/
(21) Dunbar number: https://www.backoftheenvelope.xyz/p/i...
(22) e-Estonia: https://e-estonia.com/
(23) UK wastewater monitoring of SARS-CoV-2: https://www.gov.uk/government/publica...
(24) Vitalik on Bitcoin Magazine: https://bitcoinmagazine.com/authors/v...
(25) Zero knowledge proof: https://www.infisign.ai/blog/what-is-...
(26) China banned bitcoin mining: https://forkast.news/china-banned-bit...
(27) The defi matrix: https://x.com/balajis/status/13646260...
(28) Google eats the newspaper industry: https://www.bloomberg.com/view/articl...
(29) Delaware's status as corporate capital: https://apnews.com/article/elon-musk-...
(30) Abundance: https://www.amazon.com/Abundance-Prog...
(31) Ethereum's strategic pivot:    • Ethereum's Strategic Pivot: The Plan it Ne...  
(32) AI on long tasks: https://metr.org/blog/2025-03-19-meas...
(33) The AI revolution is running out data: https://www.nature.com/articles/d4158...
(34) Detecting adversarial inputs by looking in the black box: https://ercim-news.ercim.eu/en116/spe...
(35) The rise of partisanship: https://journals.plos.org/plosone/art...
(36) China overtakes United States on contribution to research in Nature Index: https://www.nature.com/articles/d4158...
(37) Scaling ETH: https://x.com/VitalikButerin/status/1... 
(38) AI for rapid prototyping: https://x.com/balajis/status/19038209...
(39) AI taking the other AI's job: https://x.com/balajis/status/19076424...


SOCIAL
https://ethereum.foundation
https://ns.com

--

(youtube transcript)

**Balaji:** Vitalik, welcome. There's plenty to talk about. First, you want to give some remarks on the state of Ethereum? What's on your mind?

**Vitalik:** From the technical perspective, all of the pieces are finally in place to make it viable to do the kinds of things that we've been talking about doing for a really long time. I can give a few different examples of that. One of them is obviously scale. In 2017, CryptoKitties, and in 2021, DeFi, what broke all of those things is that eventually there was so much excitement it hit against the wall of fixed usage, then the transaction fees went to $50 and a bunch of people got angry.

Layer-twos are collectively doing about 250 TPS and with Pectra, the upcoming hard fork, in two weeks the blob count will double. It'll go up to 500 and then there's a pathway to increase that to about 5,000. For layer-twos, there's a pretty credible path to get to many thousands of TPS over the course of the next year or so. For the base layer, there's been this growing research direction around asking how do we super-optimize the L1, and in particular, how do we formalize some of our criteria in terms of preserving the network's decentralization, preserving the network's resilience, making sure that we're not just 25 servers, and then turning that into something where we have a very clear idea of what the constraints are so we can super-optimize around them.

There's a collection of EIPs that are planned for 2026 that look like they have a very plausible story for scaling the L1 gas limit by 10x, and then after that we of course have zkEVMs, and that's a story for scaling up even higher. In terms of scale, we've basically 10xed already. The question is how do we go further and how do we improve interoperability of things that already exist. So from a scale point of view, things that could not be done two or three years ago can be done now.

So that's scale. Another interesting dimension is security. If you think about DeFi, if you think about the question, would you confidently, with a straight face, recommend to an average person to use DeFi as a savings and wealth-building vehicle?

## Would the average person use DeFi?

**Vitalik:** I think honestly, three or four years ago, the answer just had to be an unambiguous no. And the reason basically is that what the hell is the point of even talking about 6% APY versus 4% APY when the thing that really matters to people is not getting minus 100% APY. But the thing that we've seen since then is interesting. If you look at the statistics, if you ask a bot to give you the total dollar number of DeFi hacks divided by the total number of DeFi TVL, the answer it gives is, I think, 0.53%.

So basically, in a randomly selected DeFi protocol, the chance that you'll lose money from being hacked is only half a percent. That feels a little uncomfortably high, but number one, that's only for risky protocols. 

**Balaji:** And has that been trending down over time? 

**Vitalik:** Yes, it has.

**Balaji:** That's a great graph. Can I make a comment on that?

**Vitalik:** Sure.

**Balaji:** I never did any yield farming or anything like that because you'd be "investing" with the expectation of a 6% return, but you're actually risking your entire principle because there could be a smart contract hack. You could go to zero. So instead, I'd only risk principle in an angel investment where I knew that could go to zero, and I just held back and waited till DeFi matured. And now it has started to mature.

**Vitalik:** Yeah. And for mature protocols, it's even lower than half a percent. On top of that, the other risk, of course, is the risk that you personally screw up because something happens to your wallet. This is why I've been pushing for social recovery, multisigs, account abstraction, all of this stuff non-stop for the last 10 years. The thing that did happen is the Safe front end got hacked. But if you were using an alternative UI, then you were fine. If you were checking the transactions you were signing, you were fine. And obviously, you can't expect regular people to do that. But the infrastructure keeps on hardening, and there are 10 alternative UIs that you can use. Even the base UI is planning to move to something much safer. So from an average user point of view, your ability to get something which is simultaneously decentralized and not going to lose your money is rapidly increasing.

If you remember 2017, everyone thought that smart contract wallets were dead because the wallet code itself got hacked. You remember the Parity hack, and then there was another hack. Parity got hacked twice. That was sort of the nadir for trusting smart contracts. Since then, the Safe smart contracts have actually been perfect from the start. Each and every one of these things that in the beginning phase is this crazy thing where, oh my god, there's a big chance you're going to lose all of your stuff, now it's maturing and it's more and more just actually completely fine.

**Balaji:** It's funny you say this because this is the flipping. I said this to somebody, "What is DeFi? It's what takes over after the current Western financial system ends." So there's a flipping of this is actually going to become more secure than the current Western financial system.

**Vitalik:** Yeah, exactly. This is the other side of the graph, which is the degree to which you can realistically expect confidence in TradFi. Honestly, if I had to put my money in a TradFi bank, even in the US, and just close my eyes and wait for a year, is it still there? I would say the risk that it's gone is probably a little bit higher than half a percent.

**Balaji:** Absolutely. Let's talk about this.

**Vitalik:** There's definitely people who will say it's some crazy number like 20%, and I don't think it's that high. But if you're talking about the survival of your nest egg, your retirement savings, even a freaking 2% chance is scary.

**Balaji:** That's funny because if you take your numbers, then you're saying the risk is higher. Don't invest in the dollar what you can't afford to lose.

**Vitalik:** Okay. We're starting to get more of these assets that are actually coming online. There's obviously the different US dollars, there's a bunch of different US dollars, and there's even things like Dai that are only partially dependent on actual US bank account deposits. Then you're starting to get other kinds of assets. You're also starting to get euros and other currencies. So you're able to get a pretty good diversified mirror portfolio, and your level of personal political risk basically drops down to zero. For a lot of people, for a rapidly growing percentage of the world, that's a very meaningful and large reduction in your chance of getting a minus 100% APY there.

**Balaji:** That's right. I have a few comments on this. The first is that aspect of projecting rule of law and contracts that we had talked about for a long time in crypto is now not even a reality, it's a necessity in Argentina or Nigeria or places like this. Stablecoins are actually being used all over the world. I think they flipped Visa and Mastercard not too long ago. So when people say, "Oh, what does crypto do? What are the applications of crypto?" We're like, "Well, okay, there's digital gold." Fine. But aside from that, it's bigger than Visa and Mastercard. Okay, what else you got? It's already very, very big.

**Vitalik:** Okay. So, we talked about scale, we've talked about safety. Privacy is another big one. Ethereum now has mature privacy solutions with Railgun. Now, Privacy Pools launched. I mean, now even cash is legal again.

**Balaji:** Yes.

**Vitalik:** I mean, it never really died. It actually kept being used, but it's legal even in the US to use again.

**Balaji:** We have to get Roman free. Roman Storm free and pardon Alexey. Thanks. Both free and pardon Roman, free and pardon Alexey.

**Vitalik:** Yeah. So a lot of improvements there. Also, even on the non-financial side, like if you think about things like Farcaster, I'm honestly impressed by their staying power. I think the default assumption any of us would have had a couple years ago is this is cool, this is worth supporting because it's decentralized, but it's so hard for these things to actually keep users going past the honeymoon period where they're excited.

**Balaji:** I'm super, I love Farcaster so much and we're going to do so much with Farcaster. It's basically the open social platform that we need. They've, it's like a relay race. I want to take the baton and go further with it. I can say much more about that.

**Vitalik:** It's just been running for years. It's existed for years. It has multiple clients. It has a pretty robust, pretty thriving ecosystem. A lot of really great stuff that's happening in Farcasterland. A lot of people are increasingly realizing the kind of value that it has, even aside from decentralization, just the value of a network where you can talk to interesting people and do interesting things and where people are sane. It's a pretty big deal. Both on the infrastructure side and on the technical side, I think we've just been seeing this really rapid rise in maturity over the last couple of years that I think is actually really easy to underrate. It's easy to underrate because a rise of maturity often feels like nothing is happening at all, but the thing that is happening is, of course, stuff not breaking.

**Balaji:** The reason that's so important is once something gets to 100% reliability and it's no longer interesting, then you can actually use it for something. Because you can only have so many risk tokens. If you think of a bunch of Django blocks for your startup or what have you, all of the blocks at the bottom have to be 100% like Python, Django, it'll just work. It's boring. And then you can have one risky thing on the top. But if the whole thing is risky, then it won't work. So the fact that these have gotten to that level of just, okay, yeah, it'll work, let me move on to the next scene, allows you to innovate.

**Vitalik:** Okay. So we talked about the tech, we've talked about the applications. Even prediction markets, another example, they've basically gone from theory to reality.

## Prediction markets from theory to reality

**Vitalik:** The thing that I love about prediction markets is how Polymarket was the number one app in the app store. It gets mainstream support and attention even from the types of people who normally think crypto is a scam. The types of US political intellectuals that normally just dismiss crypto and think of it as failed, they are the same people retweeting Polymarket screenshots. That's also been probably the first breakout other than classical DeFi that we've seen in a long time.

**Balaji:** Another one that's interesting is OpenRouter. Have you guys seen OpenRouter? It lets you use different LLMs and it just has MetaMask login as one of the options because it lets you pay for everything. That's like crypto as a tool as opposed to crypto hitting you in the face all the time.

**Vitalik:** Yeah. It provides actual useful value. You want to be able to talk to LLMs without them remembering everything about you.

**Balaji:** So on that, connecting that to Ethereum, one thing that's very, very rough, and I might call this label extremely approximate, is you're kind of roughly the center-left of crypto, I'm roughly the center-right of crypto, and we're both like centrists. Okay. So one way of thinking about this is I think Ethereum, you talk about the technology of it, but I think from the community standpoint... have you guys heard of Ezra Klein and Derek Thompson's abundance agenda thing?

**Vitalik:** Okay, so I think that their thing about the abundance agenda was based on a concept of like Bloomberg Democrats technocratic people as opposed to kind of the sort of riots and fires and so on that are happening now but I think that the sort of central left abundance agenda kind of people should look at Ethereum because I think Ethereum can make a play for being the technocratic Bloomberg center left of the, you know, the the let's say postwestern or internet world. There's a lot of these sort of centrist center-left people and if they were actually in control of the left, then we wouldn't have a problem. But they're not, right? And they're putting out these things and they're trying to take control of the US government or they think they are. They're not going to be able to do it. In fact, it's going to go crazier. It's going to the Luigi left, not the not the abundance agenda. Okay? And so because it's happening in my view they have to start thinking different and they have to start thinking about the other piece which is startup societies right so Ethereum is also into startup societies obviously you have Zuzulu and all the sons and daughters and grandchildren Zuzulu zoo Thailand zoo Georgia zoo this zoo that right um and um the uh the all of those things are basically places where you can experiment with different forms of governance. Those are all abundance agenda, sympathetic people and so they could actually go and do that kind of thing there. And so I think the technocratic center lefts like the Noah Smiths, Ezra Klein's, David Shores, Derek Thompson's should really lean into Ethereum, right? And I think that's actually like a social or community aspect because that's not crypto as a scam. That is crypto as actually the opposite of it. It's actually crypto as community. It's crypto as rule of law. It's crypto as you know like equality of treatment where everybody's appear on the internet.