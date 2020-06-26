import React from 'react'

export const Info = ({ bot, active }) => (
  <div className={`tab-pane pl-4 pr-4 ${active ? 'active' : ''}`} id="info" role="tabpanel" aria-labelledby="info-tab">
    <p className="mt-2"><b>Exchanges</b></p>
    <p>Deltabadger works with cryptoexchanges. At the moment, we support <a href="https://auth.bitbay.net/ref/Hhb7ZrAv2GrA" target="_blank" rel="noopener" title="Bitbay">Bitbay</a> and <a href="https://r.kraken.com/deltabadger" target="_blank" rel="noopener" title="Kraken">Kraken</a>. We recommend Kraken as a reputable exchange that has never been hacked. Use Bitbay if you want to make your purchases with PLN.</p>
    <p>To make your bot useful, first, you need to verify your exchange account and transfer some funds there.</p>
    <p>However, don't keep your coins at any exchange too long. Login to your account and do regularly withdraws at least once a month.</p>
    <p>We will add more exchanges in the future. Let us know if you are interested in a particular one.</p>
    <p className="mt-2"><b>"Smart intervals"</b></p>
    <p>Sometimes, you may be surprised that the bot schedule is different from the one you defined.</p>
    <p>Every exchange has a minimum transaction size limit. Some exchanges define it in BTC, so the USD value changes over time. If the amount you want to buy periodically is smaller, Deltabadger keeps your desired ratio using the minimum allowed size but adjusting the time interval. It protects bot from being stopped when the price of Bitcoin goes "too high," and your defined amount falls below the threshold.</p>
    <p>You can force <em>smart intervals</em> by setting small transaction size intentionally. In that way, you get the smoother averaging possible, so why not? Let's say the current limit is $20. If you set the transaction size for $24/day, the bot will do exactly that. However, if you define it as $1/hour it will mimic the ratio using the allowed size, what in that case means $20 every 20 hours. On average, it results in the same ratio ($24/day = $1/hour = $20/20hours).</p>
    <p className="mt-2"><b>Links</b></p>
    <ul className="mb-5">
      <li><a href="https://www.youtube.com/watch?v=dltaIrhUUvY" target="_blank" rel="noopener">DCA for Beginners (video)</a></li>
      <li><a href="https://medium.com/predict/the-power-of-dollar-cost-averaging-into-bitcoin-2fad7fb12ce6" target="_blank" rel="noopener">The power of DCA into Bitcoin</a></li>
      <li><a href="https://dcabtc.com/" target="_blank" rel="noopener">DCA Simulator</a></li>
    </ul>
  </div>
)
